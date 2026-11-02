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
  description = "The cedar-contrib image to run, e.g. europe-west1-docker.pkg.dev/<project>/cedar-contrib/cedar-contrib:0.3.6."
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

variable "judge_url" {
  type        = string
  default     = ""
  description = "An OpenAI-compatible chat-completions base URL the screener asks about trajectories (Vertex AI's OpenAI endpoint, or a vLLM on Cloud Run). Empty: deterministic checks only, injected results are held for review."
}

variable "judge_model" {
  type        = string
  default     = "judge"
  description = "The model name at judge_url."
}

variable "judge_api_key" {
  type        = string
  default     = ""
  sensitive   = true
  description = "The bearer token for judge_url, kept in Secret Manager. Empty: none sent."
}

variable "review_token" {
  type        = string
  default     = ""
  sensitive   = true
  description = "The token that opens the review page at /review, kept in Secret Manager. Empty: no review page."
}

variable "aging_days" {
  type        = number
  default     = 14
  description = "How long an accepted bundle waits before a release may take it."
}

variable "ablation_hook" {
  type        = string
  default     = ""
  description = "The command the ablation job runs per training set (see contrib/README.md, Ablation). Empty: no ablation job."
}

variable "edge" {
  type        = bool
  default     = false
  description = "Put a global load balancer with Cloud Armor (rate limit, WAF) in front of the service (edge.tf)."
}

variable "edge_domain" {
  type        = string
  default     = "contrib.example.org"
  description = "The hostname the edge answers on; a managed certificate is issued for it."
}
