variable "example_rocket_env_vars" {
  default = [
        {
          "name": "ROCKET_SECRET_KEY",
          "value": ""
        },
        {
          "name": "CONFIG_RECURSE_CLIENT_ID",
          "value": ""
        },
        {
          "name": "CONFIG_RECURSE_SECRET",
          "value": ""
        },
        {
          "name": "CONFIG_GITHUB_SECRET",
          "value": ""
        },
        {
          "name": "ROCKET_OAUTH",
          "value": "{recurse = { auth_uri = \"\", token_uri = \"\", client_id = \"\", client_secret = \"\", redirect_uri = \"\" }}"
        }
      ]
}