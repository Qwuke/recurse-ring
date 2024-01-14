use rocket::{
    http::{Cookie, CookieJar, SameSite},
    fairing::{ Fairing, AdHoc},
    response::{Redirect, Debug}
};
use rocket_oauth2::{OAuth2, TokenResponse};
use reqwest::Client;
use anyhow::{anyhow, Error, Context};

use crate::User;

pub fn recurse_oauth_fairing() -> impl Fairing {
    AdHoc::on_ignite("Recurse OAuth2", |rocket| async {
        rocket
            .mount("/", rocket::routes![login, callback])
            .attach(OAuth2::<User>::fairing("recurse"))
    })
}

#[get("/auth/callback")]
async fn callback(token: TokenResponse<User>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<Error>> {
    let access_token = token.access_token().to_owned();
    let response = Client::new()
        .get(format!("{}profiles/me", crate::RECURSE_BASE_URL))
        .bearer_auth(access_token.clone())
        .send().await
        .context("Unable to build reqwest client")?;

    if !response.status().is_success() {
        return Err(Debug(anyhow!("OAuth callback returned non-success status: {}", response.status())));
    }

    let decoded_content = response.text().await
        .context("Unable to decode content from OAuth callback")
        .map_err(Debug::from)?;

    let recurse_user: User = serde_json::from_str(&decoded_content)
        .context("Unable to deserialize user from OAuth callback")
        .map_err(Debug::from)?;

    cookies.add_private(
        Cookie::build(("name", recurse_user.name))
            .same_site(SameSite::Lax)
            .build());

    cookies.add_private(
        Cookie::build(("id", recurse_user.id.to_string()))
            .same_site(SameSite::Lax)
            .build());

    cookies.add_private(
        Cookie::build(("api_token", access_token))
            .same_site(SameSite::Lax)
            .build());

    Ok(Redirect::to("/"))
}

#[get("/auth/login")]
fn login(oauth2: OAuth2<User>, cookies: &CookieJar<'_>) -> Result<Redirect, Debug<Error>> {
    oauth2.get_redirect(cookies, &[])
        .context("OAuth2 unable to create a redirect given expected cookies and config")
        .map_err(Debug::from)
}