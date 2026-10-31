variable "project" {
  type        = string
  description = "The Google Cloud project id."
}

variable "region" {
  type        = string
  default     = "europe-west1"
  description = "Where Cloud Run and the image registry live."
}

variable "bucket" {
  type        = string
  description = "The quarantine bucket's name (globally unique)."
}

variable "bucket_location" {
  type        = string
  default     = "EU"
  description = "The bucket's location; EU keeps contributions in the EU jurisdiction."
}

variable "image" {
  type        = string
  description = "The cedar-contrib image to run, e.g. europe-west1-docker.pkg.dev/<project>/cedar-contrib/cedar-contrib:0.3.5."
}

variable "models" {
  type        = list(string)
  default     = ["deepseek-ai/*"]
  description = "Models whose outputs this corpus publishes: exact names or prefixes ending in *."
}

variable "min_instances" {
  type        = number
  default     = 0
  description = "1 keeps one instance warm (no cold start for the first upload of the day); 0 is the frugal setting."
}
