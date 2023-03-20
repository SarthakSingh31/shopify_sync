use worker::{D1Database, Method, Request, RequestInit, Response, RouteContext};

use crate::{fetch, Customer, Token, DB_BINDING};

#[derive(Debug, serde::Deserialize)]
struct LineItem {
    title: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Order {
    id: f64,
    customer: Customer,
    line_items: Vec<LineItem>,
}

impl Order {
    pub async fn insert_in_db(&self, db: &D1Database, shop: &str) -> worker::Result<()> {
        db.exec(&format!(
            "INSERT INTO Orders VALUES ({}, {}, {}, {}, '{}');",
            self.id,
            if let Some(name) = &self.customer.first_name {
                format!("'{name}'")
            } else {
                "NULL".to_string()
            },
            if let Some(name) = &self.customer.last_name {
                format!("'{name}'")
            } else {
                "NULL".to_string()
            },
            if let Some(email) = &self.customer.email {
                format!("'{email}'")
            } else {
                "NULL".to_string()
            },
            shop,
        ))
        .await?;

        for item in &self.line_items {
            db.prepare(format!("INSERT INTO LineItems VALUES (?, {});", self.id))
                .bind(&[item.title.as_str().into()])?
                .all()
                .await?;
        }
        Ok(())
    }

    pub async fn handle_webhook<'a, D: 'a>(
        mut req: Request,
        ctx: RouteContext<D>,
    ) -> worker::Result<Response> {
        let order: Order = req.json().await?;
        let shop = ctx.param("store").expect("Failed to find store param");

        let db = ctx.env.d1(DB_BINDING)?;

        order.insert_in_db(&db, shop).await?;

        Response::ok("ok")
    }
}

#[derive(serde::Deserialize)]
pub struct Orders {
    orders: Vec<Order>,
}

impl Orders {
    pub async fn fetch(token: &Token, shop: &str) -> worker::Result<Self> {
        let mut resp = fetch(
            token,
            Request::new_with_init(
                &format!("https://{shop}/admin/api/2023-01/orders.json?financial_status=paid&fields=id,customer,line_items&limit=250"),
                &RequestInit::default(),
            )?,
        )
        .await?;

        let mut orders: Orders = resp.json().await?;

        let mut link = resp.headers().get("Link")?;

        while let Some(url) = link {
            let mut resp = fetch(token, Request::new(&url, Method::Get)?).await?;

            let next_orders: Orders = resp.json().await?;
            orders.orders.extend(next_orders.orders);

            link = resp.headers().get("Link")?;
        }

        Ok(orders)
    }

    pub async fn insert_in_db(&self, db: &D1Database, shop: &str) -> worker::Result<()> {
        let orders = self
            .orders
            .iter()
            .map(|order| {
                format!(
                    "({}, {}, {}, {}, '{}')",
                    order.id,
                    if let Some(name) = &order.customer.first_name {
                        format!("'{name}'")
                    } else {
                        "NULL".to_string()
                    },
                    if let Some(name) = &order.customer.last_name {
                        format!("'{name}'")
                    } else {
                        "NULL".to_string()
                    },
                    if let Some(email) = &order.customer.email {
                        format!("'{email}'")
                    } else {
                        "NULL".to_string()
                    },
                    shop,
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        if !orders.is_empty() {
            db.exec(&format!("INSERT INTO Orders VALUES {};", orders))
                .await?;
        }

        let line_items = self
            .orders
            .iter()
            .flat_map(|order| {
                order
                    .line_items
                    .iter()
                    .map(|item| format!("('{}', {})", item.title, order.id))
            })
            .collect::<Vec<_>>()
            .join(",");

        if !line_items.is_empty() {
            db.exec(&format!("INSERT INTO LineItems VALUES {};", line_items))
                .await?;
        }

        Ok(())
    }
}
