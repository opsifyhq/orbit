//! Print the OpenAPI document without starting the API or a database.
//!
//! `pnpm api:generate` runs this to refresh `packages/api-client/openapi.json`, from
//! which the TypeScript client types are generated.

use utoipa::OpenApi;

fn main() {
    let spec = orbit_api::openapi::ApiDoc::openapi()
        .to_pretty_json()
        .expect("the OpenAPI document serializes");
    println!("{spec}");
}
