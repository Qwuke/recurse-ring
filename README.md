# Recurse Center Webring

The Recurse Web Ring is a hosted web service for *automatically* linking websites in a ring data structure, and optionally provides a JavaScript file for allowing sites to link directly to their ring neighbor. The hosted option allows automatic ring neighbor updates for sites and users that don't run any JavaScript, especially as a fallback, while the JavaScript file option allows users to hover over webring links before deciding to click to their neighbors.

## Rules

1. You must be a Recurse Center alumnus, own the site to some reasonable degree, and it must not be used exclusively for commercial activity such as advertising. You may include multiple sites, and they do not necessarily need to be blogs!

2. Your site must include the links provided when adding your site (Add, Prev, Home) in some visible way on your page. Random is optional, and these links may be styled _anyway_ you please. Consider browsing the webring for style inspiration.

3. Your site must abide by the Recurse Center [CoC](https://www.recurse.com/code-of-conduct) and [Social Rules](https://www.recurse.com/social-rules).

Currently, the webring's only Custodian is [Qwuke](https://github.com/Qwuke) - however, if you are a Recurse alumnus and are interested in doing some occasional ring cleanup, feel free to ask for a "promotion".

## Development and deployment

The web ring backend service is a Rust [rocket](https://rocket.rs/) web service that is deployed to AWS via terraform. 

Anything missing? **[Please consider opening an issue!](https://github.com/Qwuke/recurse-ring/issues/new?template=Blank+issue)**

### Local

The service may be run locally by creating a `docker-compose.yaml` file with the environment variables set from the [docker compose example file](https://github.com/Qwuke/recurse-ring/blob/main/docker-compose.example.yaml), and the service should run on port 4000. You may also run it by passing those environment variables to `cargo run` in the local directory, but this fails in some operating systems.

### AWS

The service can be deployed to AWS after installing the [AWS CLI](https://github.com/aws/aws-cli/tree/v2) and adding functional AWS credentials. 

Then, create a `environment.tf` file with the environment variables set from the [terraform env example file](https://github.com/Qwuke/recurse-ring/blob/main/docker-compose.example.yaml).

Finally, run `terraform init`, then `terraform plan`, and finally `terraform apply`.

