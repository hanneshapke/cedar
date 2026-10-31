# cedar-contrib on Google Cloud: the ingest service on Cloud Run, its quarantine bucket, its
# service account, the secret its withdraw tokens are derived from, and the logging exclusions
# that keep the no-request-logs promise (docs/design/contrib-ingest-cloud-run.md, §2 and §9).
#
# This is the frugal variant: Cloud Run exposed directly, with the in-memory per-client limit
# in the service. The load balancer and Cloud Armor from the design go in front of it when
# floods become real (see README.md).

terraform {
  required_version = ">= 1.6"
  required_providers {
    google = { source = "hashicorp/google", version = "~> 6.0" }
  }
}

provider "google" {
  project = var.project
  region  = var.region
}

resource "google_project_service" "apis" {
  for_each = toset(["run.googleapis.com", "storage.googleapis.com", "secretmanager.googleapis.com", "artifactregistry.googleapis.com", "logging.googleapis.com"])
  service            = each.value
  disable_on_destroy = false
}

# -- the quarantine bucket -----------------------------------------------------------------

resource "google_storage_bucket" "quarantine" {
  name                        = var.bucket
  location                    = var.bucket_location
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  # No versioning: a withdrawal must be a real delete.
  versioning { enabled = false }
  lifecycle_rule {
    # Rejected bundles are kept 30 days for the audit trail, then go.
    condition {
      age            = 30
      matches_prefix = ["bundles/"]
      # The screener sets this metadata; objects without it are untouched.
      # (Custom-metadata conditions are not supported by lifecycle rules; the release job
      # deletes rejected objects itself and this rule is the backstop for forgotten ones.)
    }
    action { type = "Delete" }
  }
  depends_on = [google_project_service.apis]
}

# -- the service account --------------------------------------------------------------------

resource "google_service_account" "api" {
  account_id   = "contrib-api"
  display_name = "cedar-contrib ingest"
}

# Objects in the quarantine bucket, and nothing else in the project.
resource "google_storage_bucket_iam_member" "api_objects" {
  bucket = google_storage_bucket.quarantine.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.api.email}"
}

# -- the withdraw-token key -----------------------------------------------------------------

resource "random_password" "hmac" {
  length  = 48
  special = false
}

resource "google_secret_manager_secret" "hmac" {
  secret_id = "contrib-hmac-key"
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "hmac" {
  secret      = google_secret_manager_secret.hmac.id
  secret_data = random_password.hmac.result
}

resource "google_secret_manager_secret_iam_member" "api_reads_hmac" {
  secret_id = google_secret_manager_secret.hmac.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.api.email}"
}

# -- the image ------------------------------------------------------------------------------

resource "google_artifact_registry_repository" "images" {
  location      = var.region
  repository_id = "cedar-contrib"
  format        = "DOCKER"
  depends_on    = [google_project_service.apis]
}

# -- Cloud Run ------------------------------------------------------------------------------

resource "google_cloud_run_v2_service" "api" {
  name     = "contrib-api"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  template {
    service_account = google_service_account.api.email
    scaling {
      min_instance_count = var.min_instances
      max_instance_count = 10
    }
    max_instance_request_concurrency = 80
    timeout                          = "60s"
    containers {
      image = var.image
      resources {
        limits            = { cpu = "1", memory = "512Mi" }
        cpu_idle          = true
        startup_cpu_boost = true
      }
      env {
        name  = "CONTRIB_BUCKET"
        value = google_storage_bucket.quarantine.name
      }
      env {
        name  = "CONTRIB_MODELS"
        value = join(",", var.models)
      }
      env {
        name = "CONTRIB_HMAC_KEY"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.hmac.secret_id
            version = "latest"
          }
        }
      }
      startup_probe {
        http_get { path = "/healthz" }
        initial_delay_seconds = 1
        period_seconds        = 3
      }
    }
  }
  depends_on = [google_secret_manager_secret_iam_member.api_reads_hmac, google_storage_bucket_iam_member.api_objects]
}

# Anyone may call it: the checks are the service's, not IAM's.
resource "google_cloud_run_v2_service_iam_member" "public" {
  name     = google_cloud_run_v2_service.api.name
  location = google_cloud_run_v2_service.api.location
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# -- no request logs ------------------------------------------------------------------------
#
# Cloud Run writes a request log line, with the client's address, for every request unless the
# project's sink excludes it. The service's own lines name a receipt id and a verdict, nothing
# else; they stay.

resource "google_logging_project_exclusion" "no_request_logs" {
  name        = "contrib-no-request-logs"
  description = "cedar-contrib keeps no request logs (they carry client addresses)"
  filter      = "resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"contrib-api\" AND logName:\"requests\""
}

# Data-access audit logs for Storage would record every object read with a principal; they are
# off by default and must stay off. (There is no resource to assert a default; the README's
# checklist covers it.)

# -- the jobs: screener every 15 minutes, release monthly -----------------------------------
#
# Both run the same image with a subcommand. The screener needs the judge's address and key
# when one is used; the release job the manifest-signing key. Cloud Scheduler starts them
# through the Run Admin API with the scheduler's own service account.

resource "google_project_service" "jobs_apis" {
  for_each           = toset(["cloudscheduler.googleapis.com"])
  service            = each.value
  disable_on_destroy = false
}

resource "google_secret_manager_secret" "release" {
  secret_id = "contrib-release-key"
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "random_password" "release" {
  length  = 48
  special = false
}

resource "google_secret_manager_secret_version" "release" {
  secret      = google_secret_manager_secret.release.id
  secret_data = random_password.release.result
}

resource "google_secret_manager_secret_iam_member" "api_reads_release" {
  secret_id = google_secret_manager_secret.release.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.api.email}"
}

locals {
  job_env = {
    CONTRIB_BUCKET     = google_storage_bucket.quarantine.name
    CONTRIB_MODELS     = join(",", var.models)
    CONTRIB_JUDGE_URL  = var.judge_url
    CONTRIB_JUDGE_MODEL = var.judge_model
    CONTRIB_AGING_DAYS = tostring(var.aging_days)
  }
}

resource "google_cloud_run_v2_job" "screener" {
  name     = "contrib-screener"
  location = var.region
  template {
    template {
      service_account = google_service_account.api.email
      timeout         = "1200s"
      max_retries     = 1
      containers {
        image = var.image
        args  = ["screen"]
        resources {
          limits = { cpu = "1", memory = "1Gi" }
        }
        dynamic "env" {
          for_each = local.job_env
          content {
            name  = env.key
            value = env.value
          }
        }
        env {
          name = "CONTRIB_HMAC_KEY"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.hmac.secret_id
              version = "latest"
            }
          }
        }
      }
    }
  }
  depends_on = [google_secret_manager_secret_iam_member.api_reads_hmac]
}

resource "google_cloud_run_v2_job" "release" {
  name     = "contrib-release"
  location = var.region
  template {
    template {
      service_account = google_service_account.api.email
      timeout         = "3600s"
      max_retries     = 0
      containers {
        image = var.image
        args  = ["release"]
        resources {
          limits = { cpu = "2", memory = "4Gi" }
        }
        dynamic "env" {
          for_each = local.job_env
          content {
            name  = env.key
            value = env.value
          }
        }
        env {
          name = "CONTRIB_HMAC_KEY"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.hmac.secret_id
              version = "latest"
            }
          }
        }
        env {
          name = "CONTRIB_RELEASE_KEY"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.release.secret_id
              version = "latest"
            }
          }
        }
      }
    }
  }
  depends_on = [google_secret_manager_secret_iam_member.api_reads_release]
}

resource "google_service_account" "scheduler" {
  account_id   = "contrib-scheduler"
  display_name = "starts the cedar-contrib jobs"
}

resource "google_cloud_run_v2_job_iam_member" "scheduler_runs_screener" {
  name     = google_cloud_run_v2_job.screener.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${google_service_account.scheduler.email}"
}

resource "google_cloud_run_v2_job_iam_member" "scheduler_runs_release" {
  name     = google_cloud_run_v2_job.release.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${google_service_account.scheduler.email}"
}

resource "google_cloud_scheduler_job" "screener" {
  name      = "contrib-screener"
  region    = var.region
  schedule  = "*/15 * * * *"
  time_zone = "Etc/UTC"
  http_target {
    http_method = "POST"
    uri         = "https://run.googleapis.com/v2/projects/${var.project}/locations/${var.region}/jobs/${google_cloud_run_v2_job.screener.name}:run"
    oauth_token {
      service_account_email = google_service_account.scheduler.email
    }
  }
  depends_on = [google_project_service.jobs_apis]
}

resource "google_cloud_scheduler_job" "release" {
  name      = "contrib-release"
  region    = var.region
  # The 3rd of the month, 04:17 UTC: off the hour, and after the month's aging has settled.
  schedule  = "17 4 3 * *"
  time_zone = "Etc/UTC"
  http_target {
    http_method = "POST"
    uri         = "https://run.googleapis.com/v2/projects/${var.project}/locations/${var.region}/jobs/${google_cloud_run_v2_job.release.name}:run"
    oauth_token {
      service_account_email = google_service_account.scheduler.email
    }
  }
  depends_on = [google_project_service.jobs_apis]
}
