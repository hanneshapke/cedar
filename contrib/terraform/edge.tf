# The edge from the design (§2): a global external Application Load Balancer in front of the
# service, which is what makes Cloud Armor available, with a per-client rate limit and the
# preconfigured WAF rules. Off by default (`edge = false`): the frugal variant answers on Cloud
# Run's own URL with the service's in-memory limit. Turn it on when floods are real; then point
# `CHAT_CONTRIBUTIONS_URL` at the edge address and set the service's ingress to
# INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER so the direct URL stops answering.
#
# Logging stays off at every layer: the backend service's log config, and Cloud Armor's rule
# logging (which would record the addresses it rate-limits).

resource "google_compute_global_address" "edge" {
  count = var.edge ? 1 : 0
  name  = "contrib-edge"
}

resource "google_compute_region_network_endpoint_group" "edge" {
  count                 = var.edge ? 1 : 0
  name                  = "contrib-edge-neg"
  network_endpoint_type = "SERVERLESS"
  region                = var.region
  cloud_run {
    service = google_cloud_run_v2_service.api.name
  }
}

resource "google_compute_security_policy" "edge" {
  count = var.edge ? 1 : 0
  name  = "contrib-edge-policy"

  # Floods: 30 requests a minute per client, then banned for 10 minutes.
  rule {
    action   = "rate_based_ban"
    priority = 1000
    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
    rate_limit_options {
      conform_action = "allow"
      exceed_action  = "deny(429)"
      enforce_on_key = "IP"
      rate_limit_threshold {
        count        = 30
        interval_sec = 60
      }
      ban_duration_sec = 600
    }
  }

  # The preconfigured WAF rules: cheap, and the body is JSON only.
  rule {
    action   = "deny(403)"
    priority = 2000
    match {
      expr {
        expression = "evaluatePreconfiguredWaf('sqli-v33-stable', {'sensitivity': 1}) || evaluatePreconfiguredWaf('xss-v33-stable', {'sensitivity': 1}) || evaluatePreconfiguredWaf('lfi-v33-stable', {'sensitivity': 1})"
      }
    }
  }

  rule {
    action   = "allow"
    priority = 2147483647
    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
  }
}

resource "google_compute_backend_service" "edge" {
  count                 = var.edge ? 1 : 0
  name                  = "contrib-edge-backend"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = "HTTPS"
  security_policy       = google_compute_security_policy.edge[0].id
  backend {
    group = google_compute_region_network_endpoint_group.edge[0].id
  }
  log_config {
    enable = false
  }
}

resource "google_compute_url_map" "edge" {
  count           = var.edge ? 1 : 0
  name            = "contrib-edge"
  default_service = google_compute_backend_service.edge[0].id
}

resource "google_compute_managed_ssl_certificate" "edge" {
  count = var.edge ? 1 : 0
  name  = "contrib-edge-cert"
  managed {
    domains = [var.edge_domain]
  }
}

resource "google_compute_target_https_proxy" "edge" {
  count            = var.edge ? 1 : 0
  name             = "contrib-edge-proxy"
  url_map          = google_compute_url_map.edge[0].id
  ssl_certificates = [google_compute_managed_ssl_certificate.edge[0].id]
}

resource "google_compute_global_forwarding_rule" "edge" {
  count                 = var.edge ? 1 : 0
  name                  = "contrib-edge-https"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  target                = google_compute_target_https_proxy.edge[0].id
  port_range            = "443"
  ip_address            = google_compute_global_address.edge[0].address
}

output "edge_address" {
  value       = var.edge ? google_compute_global_address.edge[0].address : null
  description = "Point the edge_domain's A record here, then use https://<edge_domain> as CHAT_CONTRIBUTIONS_URL."
}
