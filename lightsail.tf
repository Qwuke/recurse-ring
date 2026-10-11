# Lightsail container service: a single Nano node (0.25 vCPU / 512 MB, $7/month)
# running the same image from ECR. Lightsail provides the HTTPS endpoint, load
# balancing and health checks, so no VPC, NAT gateway or ALB is needed.

variable "image_tag" {
  description = "ECR image tag to deploy. Lightsail resolves the tag only at deploy time, so pushing a new image needs a new tag (or -replace on the deployment) to roll out."
  type        = string
  default     = "latest"
}

resource "aws_lightsail_container_service" "ring" {
  name  = "recurse-ring"
  power = "nano"
  scale = 1

  private_registry_access {
    ecr_image_puller_role {
      is_active = true
    }
  }
}

# Lets the Lightsail service pull from the existing ECR repo.
resource "aws_ecr_repository_policy" "lightsail_pull" {
  repository = aws_ecr_repository.rocket_ecr_repo.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "AllowLightsailPull"
        Effect = "Allow"
        Principal = {
          AWS = aws_lightsail_container_service.ring.private_registry_access[0].ecr_image_puller_role[0].principal_arn
        }
        Action = [
          "ecr:BatchGetImage",
          "ecr:GetDownloadUrlForLayer",
        ]
      }
    ]
  })
}

resource "aws_lightsail_container_service_deployment_version" "ring" {
  service_name = aws_lightsail_container_service.ring.name

  container {
    container_name = "rocket"
    image          = "${aws_ecr_repository.rocket_ecr_repo.repository_url}:${var.image_tag}"
    environment    = { for env in var.my_env_variables : env.name => env.value }

    ports = {
      "4000" = "HTTP"
    }
  }

  public_endpoint {
    container_name = "rocket"
    container_port = 4000

    health_check {
      path                = "/health"
      success_codes       = "200"
      interval_seconds    = 10
      timeout_seconds     = 5
      healthy_threshold   = 2
      unhealthy_threshold = 3
    }
  }

  # The puller role needs repo access before the first deployment pulls.
  depends_on = [aws_ecr_repository_policy.lightsail_pull]
}

output "lightsail_url" {
  value = aws_lightsail_container_service.ring.url
}
