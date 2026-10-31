output "url" {
  value       = google_cloud_run_v2_service.api.uri
  description = "What a cedar-chat sets as CHAT_CONTRIBUTIONS_URL."
}

output "bucket" {
  value = google_storage_bucket.quarantine.name
}
