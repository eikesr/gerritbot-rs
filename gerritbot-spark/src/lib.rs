use std::net::SocketAddr;
use std::{error, fmt, io};

use futures::future::{self, Future};
use futures::sync::mpsc::channel;
use futures::{IntoFuture as _, Sink, Stream};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;

// mod sqs;

//
// Spark data model
//

/// Define a newtype String.
macro_rules! newtype_string {
    ($type_name:ident) => {
        #[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(transparent)]
        pub struct $type_name(pub String);

        impl std::fmt::Display for $type_name {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

/// Spark id of the user
newtype_string!(PersonId);
newtype_string!(ResourceId);
newtype_string!(Email);
newtype_string!(WebhookId);
newtype_string!(MessageId);
newtype_string!(RoomId);

#[derive(Deserialize, Serialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RoomType {
    Direct,
    Group,
}

#[derive(Deserialize, Serialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Memberships,
    Messages,
    Rooms,
}

#[derive(Deserialize, Serialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Created,
    Updated,
    Deleted,
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<chrono::DateTime<chrono::Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    chrono::DateTime::parse_from_rfc3339(&s)
        .map_err(serde::de::Error::custom)
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn serialize_timestamp<S>(
    timestamp: &chrono::DateTime<chrono::Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&timestamp.to_rfc3339())
}

#[derive(Deserialize, Serialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct Timestamp(
    #[serde(deserialize_with = "deserialize_timestamp")]
    #[serde(serialize_with = "serialize_timestamp")]
    chrono::DateTime<chrono::Utc>,
);

/// Webhook's post request from Spark API
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WebhookMessage {
    id: WebhookId,
    actor_id: PersonId,
    app_id: String,
    created: Timestamp,
    created_by: PersonId,
    pub data: Message,
    event: EventType,
    name: String,
    org_id: String,
    owned_by: String,
    resource: ResourceId,
    status: String,
    target_url: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    created: Option<Timestamp>,
    id: MessageId,
    pub person_email: Email,
    pub person_id: PersonId,
    room_id: RoomId,
    room_type: RoomType,

    // a message contained in a post does not have text loaded
    #[serde(default)]
    text: String,
    markdown: Option<String>,
    html: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct PersonDetails {
    id: PersonId,
    emails: Vec<Email>,
    display_name: String,
    nick_name: Option<String>,
    org_id: String,
    created: Timestamp,
    last_activity: Option<String>,
    status: Option<String>,
    #[serde(rename = "type")]
    person_type: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WebhookRegistration {
    name: String,
    target_url: String,
    resource: ResourceType,
    event: EventType,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Webhook {
    id: WebhookId,
    name: String,
    target_url: String,
    resource: ResourceType,
    event: EventType,
    org_id: String,
    created_by: String,
    app_id: String,
    owned_by: String,
    status: String,
    created: Timestamp,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Webhooks {
    items: Vec<Webhook>,
}

//
// Client
//

#[derive(Debug, Clone)]
pub struct Client {
    client: reqwest::r#async::Client,
    url: String,
    bot_token: String,
    bot_id: PersonId,
}

#[derive(Debug)]
pub enum Error {
    ReqwestError(reqwest::Error),
    HyperError(hyper::Error),
    // SqsError(sqs::Error),
    JsonError(serde_json::Error),
    RegisterWebhook(String),
    DeleteWebhook(String),
    IoError(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ReqwestError(ref err) => fmt::Display::fmt(err, f),
            Error::HyperError(ref err) => fmt::Display::fmt(err, f),
            //Error::SqsError(ref err) => fmt::Display::fmt(err, f),
            Error::JsonError(ref err) => fmt::Display::fmt(err, f),
            Error::RegisterWebhook(ref msg) | Error::DeleteWebhook(ref msg) => {
                fmt::Display::fmt(msg, f)
            }
            Error::IoError(ref err) => fmt::Display::fmt(err, f),
        }
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::ReqwestError(ref err) => err.description(),
            Error::HyperError(ref err) => err.description(),
            // Error::SqsError(ref err) => err.description(),
            Error::JsonError(ref err) => err.description(),
            Error::RegisterWebhook(ref msg) | Error::DeleteWebhook(ref msg) => msg,
            Error::IoError(ref err) => err.description(),
        }
    }

    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            Error::ReqwestError(ref err) => err.source(),
            Error::HyperError(ref err) => err.source(),
            // Error::SqsError(ref err) => err.source(),
            Error::JsonError(ref err) => err.source(),
            Error::RegisterWebhook(_) | Error::DeleteWebhook(_) => None,
            Error::IoError(ref err) => err.source(),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::ReqwestError(err)
    }
}

impl From<hyper::Error> for Error {
    fn from(err: hyper::Error) -> Self {
        Error::HyperError(err)
    }
}

/*
impl From<sqs::Error> for Error {
    fn from(err: sqs::Error) -> Self {
        Error::SqsError(err)
    }
}
*/

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::JsonError(err)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::IoError(err)
    }
}

impl Client {
    pub fn new(
        spark_api_url: String,
        bot_token: String,
    ) -> impl Future<Item = Self, Error = Error> {
        let bootstrap_client = Client {
            client: reqwest::r#async::Client::new(),
            url: spark_api_url,
            bot_token: bot_token,
            bot_id: PersonId(String::new()),
        };

        bootstrap_client.get_bot_id().map(|bot_id| Client {
            bot_id: bot_id,
            ..bootstrap_client
        })
    }

    /// Try to get json from the given url with basic token authorization.
    fn api_get_json<T>(&self, resource: &str) -> impl Future<Item = T, Error = Error>
    where
        for<'a> T: Deserialize<'a>,
    {
        reqwest::r#async::Client::new()
            .get(&format!("{}/{}", self.url, resource))
            .bearer_auth(&self.bot_token)
            .header(http::header::ACCEPT, "application/json")
            .send()
            .from_err()
            .and_then(|response| decode_json_body(response.into_body()))
    }

    /// Try to post json to the given url with basic token authorization.
    fn api_post_json<T>(&self, resource: &str, data: &T) -> impl Future<Item = (), Error = Error>
    where
        T: Serialize,
    {
        self.client
            .post(&format!("{}/{}", self.url, resource))
            .bearer_auth(&self.bot_token)
            .header(http::header::ACCEPT, "application/json")
            .json(&data)
            .send()
            .from_err()
            .map(|_| ())
    }

    /// Try to post json to the given url with basic token authorization.
    fn api_delete(&self, resource: &str) -> impl Future<Item = (), Error = Error> {
        self.client
            .delete(&format!("{}/{}", self.url, resource))
            .bearer_auth(&self.bot_token)
            .header(http::header::ACCEPT, "application/json")
            .send()
            .from_err()
            .map(|_| ())
    }

    fn get_bot_id(&self) -> impl Future<Item = PersonId, Error = Error> {
        self.api_get_json("people/me")
            .map(|details: PersonDetails| details.id)
    }

    fn add_webhook(&self, url: &str) -> impl Future<Item = (), Error = Error> {
        let webhook = WebhookRegistration {
            name: "gerritbot".to_string(),
            target_url: url.to_string(),
            resource: ResourceType::Messages,
            event: EventType::Created,
        };

        debug!("adding webhook: {:?}", webhook);

        self.api_post_json("webhooks", &webhook)
            .map(|()| debug!("added webhook"))
    }

    fn list_webhooks(&self) -> impl Future<Item = Webhooks, Error = Error> {
        self.api_get_json("webhooks")
    }

    fn delete_webhook(&self, id: &WebhookId) -> impl Future<Item = (), Error = Error> {
        self.api_delete(&format!("webhooks/{}", id))
            .or_else(|e| match e {
                Error::ReqwestError(ref e)
                    if e.status() == Some(http::StatusCode::NO_CONTENT)
                        || e.status() == Some(http::StatusCode::NOT_FOUND) =>
                {
                    Ok(())
                }
                _ => Err(Error::DeleteWebhook(format!(
                    "Could not delete webhook: {}",
                    e
                ))),
            })
            .map(|()| debug!("deleted webhook"))
    }

    pub fn register_webhook<'a>(self, url: &str) -> impl Future<Item = (), Error = Error> {
        let url = url.to_string();
        let delete_client = self.clone();
        let add_client = self.clone();
        self.list_webhooks()
            .map(|webhooks| futures::stream::iter_ok(webhooks.items))
            .flatten_stream()
            .filter(|webhook| {
                webhook.resource == ResourceType::Messages && webhook.event == EventType::Created
            })
            .inspect(|webhook| debug!("Removing webhook from Spark: {}", webhook.target_url))
            .for_each(move |webhook| delete_client.delete_webhook(&webhook.id))
            .and_then(move |()| add_client.add_webhook(&url))
    }

    pub fn id(&self) -> &PersonId {
        &self.bot_id
    }

    pub fn reply(&self, person_id: &PersonId, msg: &str) -> impl Future<Item = (), Error = Error> {
        let json = json!({
            "toPersonId": person_id,
            "markdown": msg,
        });
        debug!("send message to {}", person_id);
        self.api_post_json("messages", &json)
    }

    pub fn get_message(
        &self,
        message_id: &MessageId,
    ) -> impl Future<Item = Message, Error = Error> {
        self.api_get_json(&format!("messages/{}", message_id))
    }
}

#[derive(Debug, Clone)]
pub struct CommandMessage {
    pub sender_email: String,
    pub sender_id: String,
    pub command: Command,
}

#[derive(Debug, Clone)]
pub enum Command {
    Enable,
    Disable,
    ShowStatus,
    ShowHelp,
    ShowFilter,
    EnableFilter,
    DisableFilter,
    SetFilter(String),
    Unknown,
}

fn reject_webhook_request(
    request: &hyper::Request<hyper::Body>,
) -> Option<hyper::Response<hyper::Body>> {
    use hyper::{Body, Response};

    if request.uri() != "/" {
        // only accept requests at "/"
        Some(
            Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        )
    } else if request.method() != http::Method::POST {
        // only accept POST
        Some(
            Response::builder()
                .status(http::StatusCode::METHOD_NOT_ALLOWED)
                .body(Body::empty())
                .unwrap(),
        )
    } else if !request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .map(|v| v.as_bytes().starts_with(&b"application/json"[..]))
        .unwrap_or(false)
    {
        // require "content-type: application/json"
        Some(
            Response::builder()
                .status(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
    } else {
        None
    }
}

/// Decode json body of HTTP request or response.
fn decode_json_body<T, B, C, E>(body: B) -> impl Future<Item = T, Error = Error>
where
    for<'a> T: Deserialize<'a>,
    B: Stream<Item = C, Error = E>,
    C: AsRef<[u8]>,
    Error: From<E>,
{
    // TODO: find a way to avoid copying here
    body.fold(Vec::new(), |mut v, chunk| {
        v.extend_from_slice(chunk.as_ref());
        future::ok(v)
    })
    .from_err()
    .and_then(|v| serde_json::from_slice::<T>(&v).into_future().from_err())
}

pub struct RawWebhookServer<M, S>
where
    M: Stream<Item = WebhookMessage, Error = ()>,
    S: Future<Item = (), Error = hyper::Error>,
{
    /// Stream of webhook posts.
    pub messages: M,
    /// Future of webhook server. Must be run in order for messages to produce
    /// anything.
    pub server: S,
}

pub fn start_raw_webhook_server(
    listen_address: &SocketAddr,
) -> RawWebhookServer<
    impl Stream<Item = WebhookMessage, Error = ()>,
    impl Future<Item = (), Error = hyper::Error>,
> {
    use hyper::{Body, Response};
    let (message_sink, messages) = channel(1);

    info!("listening to Spark on {}", listen_address);

    // very simple webhook listener
    let server = hyper::Server::bind(&listen_address).serve(move || {
        let message_sink = message_sink.clone();

        hyper::service::service_fn_ok(move |request: hyper::Request<Body>| {
            debug!("webhook request: {:?}", request);

            if let Some(error_response) = reject_webhook_request(&request) {
                // reject requests we don't understand
                warn!("rejecting webhook request: {:?}", error_response);
                error_response
            } else {
                let message_sink = message_sink.clone();
                // now try to decode the body
                let f = decode_json_body(request.into_body())
                    .map_err(|e| error!("failed to decode post body: {}", e))
                    .and_then(|post: WebhookMessage| {
                        message_sink
                            .send(post.clone())
                            .map_err(|e| error!("failed to send post body: {}", e))
                            .map(|_| ())
                    });

                // spawn a future so all of the above actually happens
                // XXX: maybe send future over the stream instead?
                tokio::spawn(f);

                Response::new(Body::empty())
            }
        })
    });

    RawWebhookServer { messages, server }
}

pub struct WebhookServer<M, S>
where
    M: Stream<Item = Message, Error = ()>,
    S: Future<Item = (), Error = hyper::Error>,
{
    /// Stream of webhook posts.
    pub messages: M,
    /// Future of webhook server. Must be run in order for messages to produce
    /// anything.
    pub server: S,
}

pub fn start_webhook_server(
    listen_address: &SocketAddr,
    client: Client,
) -> WebhookServer<
    impl Stream<Item = Message, Error = ()>,
    impl Future<Item = (), Error = hyper::Error>,
> {
    let RawWebhookServer {
        messages: raw_messages,
        server,
    } = start_raw_webhook_server(listen_address);

    let own_id = client.id().clone();

    let messages = raw_messages
        // ignore own messages
        .filter(move |post| post.data.person_id != own_id)
        .and_then(move |post| {
            client.get_message(&post.data.id).then(|message_result| {
                future::ok(
                    message_result
                        .map_err(|e| error!("failed to fetch message: {}", e))
                        .map(Some)
                        .unwrap_or(None),
                )
            })
        })
        .filter_map(std::convert::identity);

    WebhookServer { messages, server }
}

/*
pub fn sqs_event_stream<C: SparkClient + 'static + ?Sized>(
    client: Rc<C>,
    sqs_url: String,
    sqs_region: rusoto_core::Region,
) -> Result<Box<dyn Stream<Item = CommandMessage, Error = String>>, Error> {
    let bot_id = String::from(client.id());
    let sqs_stream = sqs::sqs_receiver(sqs_url, sqs_region)?;
    let sqs_stream = sqs_stream
        .filter_map(|sqs_message| {
            if let Some(body) = sqs_message.body {
                let new_post: WebhookMessage = match serde_json::from_str(&body) {
                    Ok(post) => post,
                    Err(err) => {
                        error!("Could not parse post: {}", err);
                        return None;
                    }
                };
                Some(new_post.data)
            } else {
                None
            }
        })
        .filter(move |msg| msg.person_id != bot_id)
        .filter_map(move |mut msg| {
            debug!("Loading text for message: {:#?}", msg);
            if let Err(err) = msg.load_text(&*client) {
                error!("Could not load post's text: {}", err);
                return None;
            }
            Some(msg)
        })
        .map(|msg| msg.into_command())
        .map_err(|err| format!("Error from Spark: {:?}", err));
    Ok(Box::new(sqs_stream))
}
*/
