mod dispute;
mod order;

use std::collections::BTreeMap;

use base64::Engine;
use dispute::{Dispute, Disputes};
use order::{Order, Orders};
use time::format_description::well_known::{
    iso8601::{Config, EncodedConfig, TimePrecision},
    Iso8601,
};
use worker::{Env, Fetch, Headers, Method, Request, RequestInit, Response, RouteContext, Url};

const DB_BINDING: &'static str = "ShopifyDB";

#[worker::event(fetch)]
async fn main(req: Request, env: worker::Env, _ctx: worker::Context) -> worker::Result<Response> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    worker::Router::new()
        .get_async("/", install_request)
        .get_async("/api/auth", Token::store_token)
        .get_async("/api/sync_abandoned_checkouts", sync_abandoned_checkouts)
        .get_async("/gdpr/data_request", data_request)
        .get_async("/gdpr/data_erasure", data_erasure)
        .get_async("/gdpr/shop_erasure", shop_erasure)
        .post_async("/api/order_webhook/:store", Order::handle_webhook)
        .post_async("/api/dispute_create/:store", Dispute::handle_create_webhook)
        .post_async("/api/dispute_update/:store", Dispute::handle_update_webhook)
        .run(req, env)
        .await
}

async fn install_request<'a, D: 'a>(
    req: Request,
    ctx: RouteContext<D>,
) -> worker::Result<Response> {
    let url = req.url()?;

    if validate_hmac(ctx.env.secret("SHOPIFY_CLIENT_SECRET")?.to_string(), &url) {
        let pairs = url.query_pairs();
        let mut query = BTreeMap::default();
        for (k, v) in pairs {
            query.insert(k, v);
        }

        let mut authz_url = Url::parse(&format!("https://{}/admin/oauth/authorize", query["shop"]))
            .expect("Failed to create redirect url");
        {
            let mut pairs = authz_url.query_pairs_mut();
            pairs.append_pair(
                "client_id",
                &ctx.env.secret("SHOPIFY_CLIENT_ID")?.to_string(),
            );
            pairs.append_pair(
                "scope",
                "read_customers,read_orders,read_shopify_payments_disputes",
            );
            pairs.append_pair(
                "redirect_uri",
                &format!(
                    "{}{}",
                    ctx.env.secret("SHOPIFY_BASE_URI")?.to_string(),
                    "api/auth"
                ),
            );
            pairs.append_pair("state", &rand::random::<f32>().to_string());
        }

        Response::redirect(authz_url)
    } else {
        Response::error("Failed to validate hmac", 400)
    }
}

#[derive(serde::Deserialize)]
pub struct Token {
    access_token: String,
}

impl Token {
    async fn store_token<'a, D: 'a>(
        req: Request,
        ctx: RouteContext<D>,
    ) -> worker::Result<Response> {
        let url = req.url()?;

        if validate_hmac(ctx.env.secret("SHOPIFY_CLIENT_SECRET")?.to_string(), &url) {
            let pairs = url.query_pairs();
            let mut query = BTreeMap::default();
            for (k, v) in pairs {
                query.insert(k, v);
            }

            let re = regex::Regex::new("^[a-zA-Z0-9][a-zA-Z0-9\\-]*.myshopify.com").unwrap();
            if re.is_match(&*query["shop"]) {
                let mut authn_url = Url::parse(&format!(
                    "https://{}/admin/oauth/access_token",
                    query["shop"]
                ))
                .expect("Failed to create authn url");
                {
                    let mut pairs = authn_url.query_pairs_mut();
                    pairs.append_pair(
                        "client_id",
                        &ctx.env.secret("SHOPIFY_CLIENT_ID")?.to_string(),
                    );
                    pairs.append_pair(
                        "client_secret",
                        &ctx.env.secret("SHOPIFY_CLIENT_SECRET")?.to_string(),
                    );
                    pairs.append_pair("code", &*query["code"]);
                }

                let init = RequestInit {
                    method: Method::Post,
                    ..Default::default()
                };

                let mut resp = Fetch::Request(Request::new_with_init(authn_url.as_str(), &init)?)
                    .send()
                    .await?;

                let token: Token = resp.json().await?;

                let db = ctx.env.d1(DB_BINDING)?;
                db.prepare("INSERT INTO Stores VALUES (?, ?, NULL)")
                    .bind(&[(&*query["shop"]).into(), token.access_token.as_str().into()])?
                    .all()
                    .await?;

                init_store(token, &*query["shop"], &ctx.env).await?;

                let url = String::from_utf8(
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(&*query["host"])
                        .expect("Failed to decode host url"),
                )
                .expect("Failed to decode host url utf-8");
                let url = format!("https://{url}").parse()?;

                Response::redirect(url)
            } else {
                Response::error("Failed to validate request", 400)
            }
        } else {
            Response::error("Failed to validate hmac", 400)
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct Customer {
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
}

async fn init_store(token: Token, shop: &str, env: &Env) -> worker::Result<()> {
    let base_uri = env.secret("SHOPIFY_BASE_URI")?.to_string();

    fetch(
        &token,
        Request::new_with_init(
            &format!("https://{shop}/admin/api/2023-01/webhooks.json"),
            &RequestInit {
                body: Some(
                    serde_json::json!({
                        "webhook": {
                            "address": format!(
                                "{base_uri}{}/{}",
                                "api/order_webhook",
                                shop,
                            ),
                            "topic": "orders/paid",
                            "format": "json"
                        }
                    })
                    .to_string()
                    .into(),
                ),
                method: Method::Post,
                headers: {
                    let mut headers = Headers::default();
                    headers.append("Content-Type", "application/json")?;

                    headers
                },
                ..Default::default()
            },
        )?,
    )
    .await?;

    fetch(
        &token,
        Request::new_with_init(
            &format!("https://{shop}/admin/api/2023-01/webhooks.json"),
            &RequestInit {
                body: Some(
                    serde_json::json!({
                        "webhook": {
                            "address": format!(
                                "{base_uri}{}/{}",
                                "api/dispute_create",
                                shop,
                            ),
                            "topic": "disputes/create",
                            "format": "json"
                        }
                    })
                    .to_string()
                    .into(),
                ),
                method: Method::Post,
                headers: {
                    let mut headers = Headers::default();
                    headers.append("Content-Type", "application/json")?;

                    headers
                },
                ..Default::default()
            },
        )?,
    )
    .await?;

    fetch(
        &token,
        Request::new_with_init(
            &format!("https://{shop}/admin/api/2023-01/webhooks.json"),
            &RequestInit {
                body: Some(
                    serde_json::json!({
                        "webhook": {
                            "address": format!(
                                "{base_uri}{}/{}",
                                "api/dispute_update",
                                shop,
                            ),
                            "topic": "disputes/update",
                            "format": "json"
                        }
                    })
                    .to_string()
                    .into(),
                ),
                method: Method::Post,
                headers: {
                    let mut headers = Headers::default();
                    headers.append("Content-Type", "application/json")?;

                    headers
                },
                ..Default::default()
            },
        )?,
    )
    .await?;

    let db = env.d1(DB_BINDING)?;

    Orders::fetch(&token, shop)
        .await?
        .insert_in_db(&db, shop)
        .await?;
    Disputes::fetch(&token, shop)
        .await?
        .insert_in_db(&db, shop)
        .await?;

    Ok(())
}

async fn sync_abandoned_checkouts<'a, D: 'a>(
    _req: Request,
    ctx: RouteContext<D>,
) -> worker::Result<Response> {
    let db = ctx.env.d1(DB_BINDING)?;

    #[derive(serde::Deserialize)]
    struct Shop {
        name: String,
        access_token: String,
        last_abandoned_checkout_sync: Option<i64>,
    }

    #[derive(serde::Deserialize)]
    struct Checkout {
        id: f64,
        abandoned_checkout_url: String,
        customer: Customer,
    }

    #[derive(serde::Deserialize)]
    struct Checkouts {
        checkouts: Vec<Checkout>,
    }

    let shops = db
        .prepare("SELECT * FROM Stores;")
        .all()
        .await?
        .results::<Shop>()?;

    for shop in shops {
        let token = Token {
            access_token: shop.access_token,
        };

        let url = format!(
            "https://{}/admin/api/2023-01/checkouts.json?limit=250&status=open{}",
            shop.name,
            if let Some(datetime) = shop.last_abandoned_checkout_sync {
                const CONFIG: EncodedConfig = Config::DEFAULT
                    .set_time_precision(TimePrecision::Second {
                        decimal_digits: None,
                    })
                    .encode();

                format!(
                    "&created_at_min={}",
                    time::OffsetDateTime::from_unix_timestamp(datetime)
                        .expect("Failed to convert unix timestamp to a time")
                        .format(&Iso8601::<CONFIG>)
                        .expect("Failed to format time")
                )
            } else {
                String::default()
            }
        );

        let abandoned_checkouts = fetch(&token, Request::new(&url, Method::Get)?)
            .await?
            .json::<Checkouts>()
            .await?;

        let last_abandoned_checkout_sync = time::OffsetDateTime::now_utc().unix_timestamp();
        db.exec(&format!(
            "UPDATE Stores SET last_abandoned_checkout_sync = {} WHERE name = '{}';",
            last_abandoned_checkout_sync, shop.name
        ))
        .await?;

        let insert_tuples = abandoned_checkouts
            .checkouts
            .into_iter()
            .map(|checkout| {
                format!(
                    "({}, '{}', {}, {}, {}, '{}')",
                    checkout.id,
                    checkout.abandoned_checkout_url,
                    if let Some(name) = &checkout.customer.first_name {
                        format!("'{name}'")
                    } else {
                        "NULL".to_string()
                    },
                    if let Some(name) = &checkout.customer.last_name {
                        format!("'{name}'")
                    } else {
                        "NULL".to_string()
                    },
                    if let Some(email) = &checkout.customer.email {
                        format!("'{email}'")
                    } else {
                        "NULL".to_string()
                    },
                    shop.name
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        if !insert_tuples.is_empty() {
            db.exec(&format!(
                "INSERT INTO AbandonedCheckout VALUES {};",
                insert_tuples
            ))
            .await?;
        }
    }

    Response::ok("Done")
}

async fn data_request<'a, D: 'a>(
    mut req: Request,
    ctx: RouteContext<D>,
) -> worker::Result<Response> {
    #[derive(serde::Deserialize)]
    struct ReqBody {
        orders_requested: Vec<f64>,
        customer: Option<Customer>,
    }

    let body: ReqBody = req.json().await?;

    let db = ctx.env.d1(DB_BINDING)?;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct DbOrder {
        id: f64,
        first_name: Option<String>,
        last_name: Option<String>,
        email: Option<String>,
        store_name: String,
    }

    let orders = db
        .prepare(&format!(
            "SELECT * FROM Orders WHERE id IN {}",
            body.orders_requested
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
        .all()
        .await?
        .results::<DbOrder>()?;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct DbAbandonedCheckout {
        id: u64,
        checkout_url: String,
        first_name: Option<String>,
        last_name: Option<String>,
        email: Option<String>,
        store_name: String,
    }

    let mut abandoned_checkouts = None;
    if let Some(customer) = body.customer {
        if let Some(email) = customer.email {
            abandoned_checkouts = Some(
                db.prepare("SELECT * FROM AbandonedCheckout WHERE email = ?;")
                    .bind(&[email.into()])?
                    .all()
                    .await?
                    .results::<DbAbandonedCheckout>()?,
            );
        }
    }

    Response::from_json(
        &serde_json::json!({
            "orders": orders,
            "abandoned_checkouts": abandoned_checkouts,
        })
        .to_string(),
    )
}

async fn data_erasure<'a, D: 'a>(
    mut req: Request,
    ctx: RouteContext<D>,
) -> worker::Result<Response> {
    #[derive(serde::Deserialize)]
    struct ReqBody {
        customer: Option<Customer>,
        orders_to_redact: Vec<f64>,
    }

    let body: ReqBody = req.json().await?;

    let db = ctx.env.d1(DB_BINDING)?;

    db.exec(&format!(
        "DELETE FROM Orders WHERE id IN ({});",
        body.orders_to_redact
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    ))
    .await?;

    if let Some(customer) = body.customer {
        if let Some(email) = customer.email {
            db.prepare("DELETE FROM AbandonedCheckout WHERE email = ?;")
                .bind(&[email.into()])?
                .all()
                .await?;
        }
    }

    Response::ok("Done")
}

async fn shop_erasure<'a, D: 'a>(
    mut req: Request,
    ctx: RouteContext<D>,
) -> worker::Result<Response> {
    #[derive(serde::Deserialize)]
    struct ReqBody {
        shop_domain: String,
    }

    let body: ReqBody = req.json().await?;

    let db = ctx.env.d1(DB_BINDING)?;

    db.prepare("DELETE FROM Stores WHERE name = ?;")
        .bind(&[body.shop_domain.into()])?
        .all()
        .await?;

    Response::ok("Done")
}

fn validate_hmac<B: AsRef<[u8]>>(secret: B, url: &Url) -> bool {
    use hmac::Mac;

    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_ref())
        .expect("HMAC failed to construct");

    let mut hmac = None;
    let mut query = BTreeMap::new();

    for (k, v) in url.query_pairs() {
        match &*k {
            "hmac" => hmac = Some(v.to_string()),
            k => {
                query.insert(k.to_string(), v.to_string());
            }
        }
    }

    if let Some(hmac) = hmac {
        let query = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        mac.update(query.as_bytes());

        let result = mac.finalize();

        hex::encode(result.into_bytes()) == hmac
    } else {
        false
    }
}

async fn fetch(token: &Token, mut req: Request) -> worker::Result<Response> {
    req.headers_mut()?
        .append("X-Shopify-Access-Token", &token.access_token)?;

    Fetch::Request(req).send().await
}
