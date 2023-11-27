use std::collections::HashMap;
use std::sync::Mutex;
use serde::Deserialize;
use figment::providers::{Format, Toml};
use figment::Figment;
use rocket::{Config, FromForm, State};
use rocket::request::{FromRequest, Request, Outcome};

#[macro_use] extern crate rocket;

#[derive(Deserialize)]
struct ClientTokens {
    recurse_client_id: String,
    recurse_secret: String,
    github_secret: String,
}

#[launch]
fn rocket() -> _ {
    let config: Config = Figment::from(Toml::file("Secrets.toml")).extract().expect("Config should be extractable");

    rocket::build()
        .manage(Mutex::new(HashMap::<String, String>::new()))
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![home, add, login])
}

#[post("/auth/add")]
fn add() {
    todo!()
}

#[get("/auth/login")]
fn login() {
    todo!()
}

#[get("/")]
fn home() -> String {
    todo!()
}

#[derive(FromForm)]
pub struct QueryParams { params: HashMap<String, String> }

#[rocket::async_trait]
impl<'r> FromRequest<'r> for QueryParams {
    type Error = rocket::Error;
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let param_map = request.uri()
                            .query()
                            .unwrap()
                            .segments()
                            .map(|(k, v)| (k.to_owned(), v.to_owned()))
                            .collect();
        Outcome::Success(QueryParams { params: param_map })
    }
}
