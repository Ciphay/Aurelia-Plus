use super::*;
use crate::web::web_session::{ECON_IMAGE_BASE, WebSession};
use serde_json::Value;
use std::sync::LazyLock;
use steam_vent_proto::steammessages_auth_steamclient::{
    CAuthentication_AccessToken_GenerateForApp_Request,
    CAuthentication_AccessToken_GenerateForApp_Response,
};

#[derive(Debug, Clone, serde::Serialize)]
pub struct InventoryItem {
    pub asset_id: String,
    pub class_id: String,
    pub name: String,
    pub market_hash_name: String,
    pub item_type: String,
    pub amount: u64,
    pub tradable: bool,
    pub marketable: bool,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketPrice {
    pub market_hash_name: String,
    pub lowest_price: Option<String>,
    pub median_price: Option<String>,
    pub volume: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketSearchResult {
    pub name: String,
    pub market_hash_name: String,
    pub app_id: u32,
    pub app_name: String,
    pub sell_listings: u32,
    pub sell_price: u64,
    pub sell_price_text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MyListing {
    pub listing_id: String,
    pub market_hash_name: String,
    pub price: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MyBuyOrder {
    pub buy_order_id: String,
    pub market_hash_name: String,
    pub price: u64,
    pub quantity: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MyMarketState {
    pub listings: Vec<MyListing>,
    pub buy_orders: Vec<MyBuyOrder>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletBalance {
    pub balance_cents: i64,
    pub currency: u32,
    pub country: String,
    pub formatted: String,
}

impl SteamClient {
    pub async fn web_session(&self) -> Result<WebSession> {
        let connection = self.require_connection()?;
        let steam_id = u64::from(connection.steam_id());
        let refresh_token = connection.refresh_token().token().to_string();

        let mut req = CAuthentication_AccessToken_GenerateForApp_Request::new();
        req.set_refresh_token(refresh_token);
        req.set_steamid(steam_id);

        let resp: CAuthentication_AccessToken_GenerateForApp_Response = connection
            .service_method(req)
            .await
            .map_err(|e| anyhow!("failed to mint a web access token: {e}"))?;
        let token = resp.access_token();
        if token.is_empty() {
            bail!("Steam returned an empty web access token");
        }
        WebSession::new(
            steam_id,
            token,
            Some(connection.ip_country_code()).filter(|c| !c.is_empty()),
        )
    }
}

pub async fn inventory_via(
    web: &WebSession,
    app_id: u32,
    context_id: u32,
) -> Result<Vec<InventoryItem>> {
    let steam_id = web.steam_id();
    let url = format!(
        "https://steamcommunity.com/inventory/{steam_id}/{app_id}/{context_id}?l=english&count=2000"
    );
    parse_inventory(&web.get_text(&url).await?)
}

pub async fn my_listings_via(web: &WebSession) -> Result<MyMarketState> {
    let body = web
        .get_text("https://steamcommunity.com/market/mylistings/?norender=1&count=100")
        .await?;
    parse_my_listings(&body)
}

pub async fn wallet_via(web: &WebSession) -> Result<WalletBalance> {
    let body = web.get_text("https://steamcommunity.com/market/").await?;
    parse_wallet(&body)
}

fn public_client() -> Result<reqwest::Client> {
    crate::core::net::steam_web_client()
}

pub async fn market_price(
    app_id: u32,
    market_hash_name: &str,
    currency: u32,
) -> Result<MarketPrice> {
    let client = public_client()?;
    let request = client
        .get("https://steamcommunity.com/market/priceoverview/")
        .query(&[
            ("appid", app_id.to_string()),
            ("currency", currency.to_string()),
            ("market_hash_name", market_hash_name.to_string()),
        ]);
    let resp = crate::core::net::send_with_retry(&client, request)
        .await
        .context("market price request failed")?;
    let v: Value = resp
        .json()
        .await
        .context("invalid price-overview response")?;
    if v.get("success").and_then(Value::as_bool) != Some(true) {
        bail!(
            "no market data for '{market_hash_name}' (it may not be marketable, or the name/app id \
             is wrong — names are case-sensitive and exact)"
        );
    }
    Ok(MarketPrice {
        market_hash_name: market_hash_name.to_string(),
        lowest_price: v
            .get("lowest_price")
            .and_then(Value::as_str)
            .map(String::from),
        median_price: v
            .get("median_price")
            .and_then(Value::as_str)
            .map(String::from),
        volume: v.get("volume").and_then(Value::as_str).map(String::from),
    })
}

pub async fn market_search(
    query: Option<&str>,
    app_id: Option<u32>,
    count: u32,
) -> Result<(u32, Vec<MarketSearchResult>)> {
    let mut params: Vec<(&str, String)> = vec![
        ("norender", "1".to_string()),
        ("start", "0".to_string()),
        ("count", count.to_string()),
    ];
    if let Some(q) = query {
        params.push(("query", q.to_string()));
    }
    if let Some(a) = app_id {
        params.push(("appid", a.to_string()));
    }

    let client = public_client()?;
    let request = client
        .get("https://steamcommunity.com/market/search/render/")
        .query(&params);
    let resp = crate::core::net::send_with_retry(&client, request)
        .await
        .context("market search request failed")?;
    let v: Value = resp
        .json()
        .await
        .context("invalid market-search response")?;
    let total = v.get("total_count").and_then(Value::as_u64).unwrap_or(0) as u32;
    let results = parse_json_array(&v, "results", parse_search_result);
    Ok((total, results))
}

fn parse_json_array<T>(v: &Value, key: &str, f: impl FnMut(&Value) -> Option<T>) -> Vec<T> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(f).collect())
        .unwrap_or_default()
}

fn parse_search_result(v: &Value) -> Option<MarketSearchResult> {
    Some(MarketSearchResult {
        name: v.get("name")?.as_str()?.to_string(),
        market_hash_name: v
            .get("hash_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        app_id: v
            .get("asset_description")
            .and_then(|a| a.get("appid"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        app_name: v
            .get("app_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sell_listings: v.get("sell_listings").and_then(Value::as_u64).unwrap_or(0) as u32,
        sell_price: v.get("sell_price").and_then(Value::as_u64).unwrap_or(0),
        sell_price_text: v
            .get("sell_price_text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn as_i64_loose(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn as_u64_loose(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn parse_inventory(body: &str) -> Result<Vec<InventoryItem>> {
    let v: Value = serde_json::from_str(body).context(
        "could not parse the inventory response (the web session may be invalid — try \
         `aurelia login --reconnect`)",
    )?;
    if v.get("success").and_then(Value::as_i64) == Some(0) {
        let err = v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("inventory unavailable");
        bail!("{err}");
    }
    let Some(assets) = v.get("assets").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut descriptions: HashMap<(String, String), &Value> = HashMap::new();
    if let Some(arr) = v.get("descriptions").and_then(Value::as_array) {
        for d in arr {
            let cid = d
                .get("classid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let iid = d
                .get("instanceid")
                .and_then(Value::as_str)
                .unwrap_or("0")
                .to_string();
            descriptions.insert((cid, iid), d);
        }
    }

    let mut items = Vec::with_capacity(assets.len());
    for a in assets {
        let class_id = a
            .get("classid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let instance_id = a
            .get("instanceid")
            .and_then(Value::as_str)
            .unwrap_or("0")
            .to_string();
        let asset_id = a
            .get("assetid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let amount = a.get("amount").and_then(as_u64_loose).unwrap_or(1);

        let desc = descriptions.get(&(class_id.clone(), instance_id));
        let get = |k: &str| {
            desc.and_then(|d| d.get(k).and_then(Value::as_str))
                .unwrap_or_default()
        };
        let flag = |k: &str| {
            desc.and_then(|d| d.get(k).and_then(Value::as_i64))
                .unwrap_or(0)
                == 1
        };
        let icon = desc
            .and_then(|d| d.get("icon_url").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(|s| format!("{ECON_IMAGE_BASE}{s}"));

        items.push(InventoryItem {
            asset_id,
            class_id,
            name: get("name").to_string(),
            market_hash_name: get("market_hash_name").to_string(),
            item_type: get("type").to_string(),
            amount,
            tradable: flag("tradable"),
            marketable: flag("marketable"),
            icon_url: icon,
        });
    }
    Ok(items)
}

fn parse_my_listings(body: &str) -> Result<MyMarketState> {
    let v: Value = serde_json::from_str(body).context(
        "could not parse your market listings (the web session may be invalid — try \
         `aurelia login --reconnect`)",
    )?;
    if matches!(v.get("success").and_then(Value::as_bool), Some(false))
        || matches!(v.get("success").and_then(Value::as_i64), Some(0))
    {
        bail!(
            "the market session was rejected (no authenticated response). Run \
             `aurelia login --reconnect`."
        );
    }
    let listings = parse_json_array(&v, "listings", parse_my_listing);
    let buy_orders = parse_json_array(&v, "buy_orders", parse_my_buy_order);
    Ok(MyMarketState {
        listings,
        buy_orders,
    })
}

fn parse_my_listing(v: &Value) -> Option<MyListing> {
    let listing_id = v.get("listingid")?.as_str()?.to_string();
    let asset = v.get("asset");
    let name = asset
        .and_then(|a| {
            a.get("market_hash_name")
                .or_else(|| a.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();
    let price = v.get("price").and_then(as_u64_loose).unwrap_or(0);
    Some(MyListing {
        listing_id,
        market_hash_name: name,
        price,
    })
}

fn parse_my_buy_order(v: &Value) -> Option<MyBuyOrder> {
    let buy_order_id = v.get("buy_orderid")?.as_str()?.to_string();
    Some(MyBuyOrder {
        buy_order_id,
        market_hash_name: v
            .get("hash_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        price: v.get("price").and_then(as_u64_loose).unwrap_or(0),
        quantity: v.get("quantity").and_then(as_u64_loose).unwrap_or(0),
    })
}

fn parse_wallet(html: &str) -> Result<WalletBalance> {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"g_rgWalletInfo\s*=\s*(\{.*?\})\s*;").unwrap());
    let caps = RE.captures(html).context(
        "could not read your wallet (no `g_rgWalletInfo` on the market page). Make sure you are \
         logged in and the account is market-eligible.",
    )?;
    let json: Value = serde_json::from_str(&caps[1]).context("failed to parse wallet info")?;

    let currency = json
        .get("wallet_currency")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let country = json
        .get("wallet_country")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let balance_cents = json
        .get("wallet_balance")
        .and_then(as_i64_loose)
        .unwrap_or(0);

    Ok(WalletBalance {
        formatted: format_currency(balance_cents, currency),
        balance_cents,
        currency,
        country,
    })
}

pub fn format_currency(cents: i64, currency: u32) -> String {
    let code = currency_code(currency);
    format!("{:.2} {code}", cents as f64 / 100.0)
}

fn currency_code(currency: u32) -> String {
    match currency {
        1 => "USD",
        2 => "GBP",
        3 => "EUR",
        4 => "CHF",
        5 => "RUB",
        6 => "PLN",
        7 => "BRL",
        8 => "JPY",
        9 => "NOK",
        13 => "SGD",
        16 => "KRW",
        17 => "TRY",
        18 => "UAH",
        19 => "MXN",
        20 => "CAD",
        21 => "AUD",
        22 => "NZD",
        23 => "CNY",
        24 => "INR",
        28 => "ZAR",
        29 => "HKD",
        _ => return format!("(currency {currency})"),
    }
    .to_string()
}
