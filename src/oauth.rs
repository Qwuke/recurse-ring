use rocket::{
    http::{Cookie, CookieJar, SameSite},
    fairing::{ Fairing, AdHoc},
    response::Redirect,
};
use rocket_oauth2::{OAuth2, TokenResponse};
use reqwest::Client;

use crate::User;


pub fn recurse_oauth_fairing() -> impl Fairing {
    AdHoc::on_ignite("Recurse OAuth2", |rocket| async {
        rocket
            .mount("/", rocket::routes![login, callback])
            .attach(OAuth2::<User>::fairing("recurse"))
    })
}

#[get("/auth/callback")]
async fn callback(
    token: TokenResponse<User>,
    cookies: &CookieJar<'_>,
) -> Result<Redirect, String> {
    let access_token = token.access_token().to_owned();
    let response = Client::new()
        .get(format!("{}profiles/me", crate::RECURSE_BASE_URL))
        .bearer_auth(access_token.clone())
        .send().await.unwrap();

    if !response.status().is_success() {
        return Err(format!("Got non-success status {}", response.status()));
    }

    let user: User = serde_json::from_str(response.text().await.unwrap().as_str()).unwrap();

    cookies.add_private(
        Cookie::build(("name", user.name))
            .same_site(SameSite::Lax)
            .build(),
    );

    cookies.add_private(
        Cookie::build(("id", user.id.to_string()))
            .same_site(SameSite::Lax)
            .build(),
    );

    cookies.add_private(
        Cookie::build(("api_token", access_token))
            .same_site(SameSite::Lax)
            .build(),
    );

    Ok(Redirect::to("/"))
}

#[get("/auth/login")]
fn login(oauth2: OAuth2<User>, cookies: &CookieJar<'_>) -> Redirect {
    oauth2.get_redirect(cookies, &[]).unwrap()
}