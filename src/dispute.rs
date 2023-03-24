use worker::{D1Database, Method, Request, RequestInit, Response, RouteContext};

use crate::{fetch, Token, DB_BINDING};

#[derive(Debug, serde::Deserialize)]
pub struct Dispute {
    id: f64,
    order_id: Option<f64>,
    r#type: String,
    amount: String,
    currency: String,
    reason: String,
    status: String,
    initiated_at: String,
    evidence_due_by: String,
    evidence_sent_on: Option<String>,
}

impl Dispute {
    pub async fn handle_create_webhook<'a, D: 'a>(
        mut req: Request,
        ctx: RouteContext<D>,
    ) -> worker::Result<Response> {
        let dispute: Dispute = req.json().await?;

        let shop = ctx.param("store").expect("Failed to find store param");
        let db = ctx.env.d1(DB_BINDING)?;

        db.exec(&format!(
            "INSERT INTO Disputes VALUES ({}, {}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, '{}');",
            dispute.id,
            if let Some(order_id) = &dispute.order_id {
                order_id.to_string()
            } else {
                "NULL".to_string()
            },
            dispute.r#type,
            dispute.amount,
            dispute.currency,
            dispute.reason,
            dispute.status,
            dispute.initiated_at,
            dispute.evidence_due_by,
            if let Some(sent_on) = &dispute.evidence_sent_on {
                format!("'{sent_on}'")
            } else {
                "NULL".to_string()
            },
            shop,
        ))
        .await?;

        Response::ok("ok")
    }

    pub async fn handle_update_webhook<'a, D: 'a>(
        mut req: Request,
        ctx: RouteContext<D>,
    ) -> worker::Result<Response> {
        let dispute: Dispute = req.json().await?;

        let db = ctx.env.d1(DB_BINDING)?;

        db.exec(&format!(
            "UPDATE Disputes SET order_id = {}, type = '{}', amount = '{}', currency = '{}', reason = '{}', status = '{}', evidence_due_by = '{}', evidence_sent_on = '{}', evidence_sent_on = {} WHERE id = {};",
            if let Some(order_id) = &dispute.order_id {
                order_id.to_string()
            } else {
                "NULL".to_string()
            },
            dispute.r#type,
            dispute.amount,
            dispute.currency,
            dispute.reason,
            dispute.status,
            dispute.initiated_at,
            dispute.evidence_due_by,
            if let Some(sent_on) = &dispute.evidence_sent_on {
                format!("'{sent_on}'")
            } else {
                "NULL".to_string()
            },
            dispute.id,
        ))
        .await?;

        Response::ok("ok")
    }
}

#[derive(serde::Deserialize)]
pub struct Disputes {
    disputes: Vec<Dispute>,
}

impl Disputes {
    pub async fn fetch(token: &Token, shop: &str) -> worker::Result<Self> {
        let mut resp = fetch(
            token,
            Request::new_with_init(
                &format!(
                    "https://{shop}/admin/api/2023-01/shopify_payments/disputes.json?limit=250"
                ),
                &RequestInit::default(),
            )?,
        )
        .await?;

        // When there are no disputes it returns an empty body with content-type html
        if !resp
            .headers()
            .get("content-type")?
            .unwrap_or_default()
            .contains("application/json")
        {
            return Ok(Disputes {
                disputes: Vec::default(),
            });
        }
        let mut disputes: Disputes = resp.json().await?;

        let mut link = resp.headers().get("Link")?;

        while let Some(url) = link {
            let mut resp = fetch(token, Request::new(&url, Method::Get)?).await?;

            let next_disputes: Disputes = resp.json().await?;
            disputes.disputes.extend(next_disputes.disputes);

            link = resp.headers().get("Link")?;
        }

        Ok(disputes)
    }

    pub async fn insert_in_db(&self, db: &D1Database, shop: &str) -> worker::Result<()> {
        let disputes = self
            .disputes
            .iter()
            .map(|dispute| {
                format!(
                    "({}, {}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, '{}')",
                    dispute.id,
                    if let Some(order_id) = &dispute.order_id {
                        order_id.to_string()
                    } else {
                        "NULL".to_string()
                    },
                    dispute.r#type,
                    dispute.amount,
                    dispute.currency,
                    dispute.reason,
                    dispute.status,
                    dispute.initiated_at,
                    dispute.evidence_due_by,
                    if let Some(sent_on) = &dispute.evidence_sent_on {
                        format!("'{sent_on}'")
                    } else {
                        "NULL".to_string()
                    },
                    shop,
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        if !disputes.is_empty() {
            db.exec(&format!("INSERT INTO Disputes VALUES {};", disputes))
                .await?;
        }

        Ok(())
    }
}

