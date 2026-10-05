//! A `reqwest` replacement for Windows Phone 8.1.
//!
//! # Why this cannot be the real crate
//!
//! `phira` reaches the Phira API through `reqwest`, and with the `rustls` feature it pulls
//! `reqwest -> rustls -> aws-lc-sys`. `aws-lc-sys` is a C library whose build script announces
//!
//! ```text
//! Target platform: 'thumbv7a-pc-windows-msvc'
//! No target-specific source found: windows-arm
//! ```
//!
//! and then fails compiling `stdalign_check.c` with `cl.exe` (exit code 2). There is no source for
//! this architecture, so the crate cannot be built here at all - not "is awkward to build".
//!
//! That is the first reason, and not the important one. The port's rule is that every network
//! operation belongs to the C# shell (network, login and online charts run through `HostCallbacks`)
//! precisely because `reqwest` brings its own sockets and runtime. Even had `aws-lc-sys` built we
//! would be replacing this crate.
//!
//! # Why the surface is complete rather than minimal
//!
//! A stub covering only the calls that were noticed fails at the next unseen call site, and each
//! miss costs a full cross-compilation round trip - minutes, not seconds, because `-Zbuild-std`
//! recompiles the standard library for the target. So everything `phira` and `phira-monitor` name
//! is here: `Client`, `ClientBuilder`, `RequestBuilder`, `Response`, `StatusCode`, `Method`,
//! `Url`, `header::*` and `redirect::Policy`. The parts that would need a real transport are the
//! ones that return errors.
//!
//! # What is not implemented, and why not yet
//!
//! [`RequestBuilder::send`] returns an error. Wiring it means adding an `http_request` entry to
//! `prpr-port`'s `HostCallbacks` and implementing it in C# with `HttpClient` - and that has to be
//! done as a *threaded* callback, because the futures here are polled from the render loop on the
//! UI thread, where a synchronous `.Result` on a `HttpClient` call deadlocks. Doing it properly is
//! a milestone of its own; the UI must not wait for it.
//!
//! Until then `phira` renders normally and network-backed parts of it report failure - which is
//! also the honest state of the port.

#![allow(unused_variables)]

use std::fmt;

use bytes::Bytes;
use serde::{de::DeserializeOwned, Serialize};

pub use url::Url;

/// The alias upstream exposes. Kept so a `use reqwest::Result` resolves.
pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Stands in for `reqwest::Error`.
///
/// Only the message and an optional status are carried: the classification accessors
/// (`is_timeout`, `is_connect`, ...) are all answered honestly as `false` for transport failures
/// we never performed, but they have to *exist* because error paths in `phira` branch on them and
/// a missing method there would otherwise surface as another full rebuild.
#[derive(Debug, Clone)]
pub struct Error {
    msg: String,
    status: Option<StatusCode>,
    url: Option<Url>,
}

impl Error {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Error {
            msg: msg.into(),
            status: None,
            url: None,
        }
    }

    pub(crate) fn transport(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    pub fn status(&self) -> Option<StatusCode> {
        self.status
    }

    pub fn url(&self) -> Option<&Url> {
        self.url.as_ref()
    }

    pub fn is_builder(&self) -> bool {
        false
    }
    pub fn is_request(&self) -> bool {
        false
    }
    pub fn is_redirect(&self) -> bool {
        false
    }
    pub fn is_timeout(&self) -> bool {
        false
    }
    pub fn is_connect(&self) -> bool {
        false
    }
    pub fn is_body(&self) -> bool {
        false
    }
    pub fn is_decode(&self) -> bool {
        false
    }
    pub fn is_status(&self) -> bool {
        self.status.is_some()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// header
// ---------------------------------------------------------------------------

pub mod header {
    use std::{borrow::Cow, fmt};

    /// Case-insensitive header name.
    ///
    /// `Cow<'static, str>` rather than `String` because every name upstream exposes as a constant
    /// (`ACCEPT_LANGUAGE`, `AUTHORIZATION`, ...) is a literal, and those must be constructible in a
    /// `const` item - which rules out a function call and therefore `String`.
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct HeaderName(Cow<'static, str>);

    impl HeaderName {
        pub fn from_static(s: &'static str) -> Self {
            HeaderName(Cow::Borrowed(s))
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    impl fmt::Display for HeaderName {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    pub const ACCEPT: HeaderName = HeaderName(Cow::Borrowed("accept"));
    pub const ACCEPT_LANGUAGE: HeaderName = HeaderName(Cow::Borrowed("accept-language"));
    pub const AUTHORIZATION: HeaderName = HeaderName(Cow::Borrowed("authorization"));
    pub const CONTENT_LENGTH: HeaderName = HeaderName(Cow::Borrowed("content-length"));
    pub const CONTENT_TYPE: HeaderName = HeaderName(Cow::Borrowed("content-type"));
    pub const IF_MODIFIED_SINCE: HeaderName = HeaderName(Cow::Borrowed("if-modified-since"));
    pub const LAST_MODIFIED: HeaderName = HeaderName(Cow::Borrowed("last-modified"));
    pub const LOCATION: HeaderName = HeaderName(Cow::Borrowed("location"));
    pub const USER_AGENT: HeaderName = HeaderName(Cow::Borrowed("user-agent"));

    /// Error from `HeaderValue::from_str`.
    ///
    /// Has to be `Send + Sync + 'static` and implement `std::error::Error`: call sites pass it
    /// straight to `?` inside functions returning `anyhow::Result`, and that bound is what the
    /// conversion requires.
    #[derive(Debug)]
    pub struct InvalidHeaderValue {
        name: &'static str,
        value: String,
    }

    impl fmt::Display for InvalidHeaderValue {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "invalid header value {:?} for {}", self.value, self.name)
        }
    }

    impl std::error::Error for InvalidHeaderValue {}

    /// Error from `HeaderValue::to_str`.
    #[derive(Debug)]
    pub struct ToStrError;

    impl fmt::Display for ToStrError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("header value contains invalid characters")
        }
    }

    impl std::error::Error for ToStrError {}

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct HeaderValue(Cow<'static, str>);

    impl HeaderValue {
        /// For literal values, avoiding the allocation `from_str` has to make.
        pub fn from_static(v: &'static str) -> HeaderValue {
            HeaderValue(Cow::Borrowed(v))
        }

        /// Rejects control characters, matching upstream closely enough that a value the real
        /// crate would refuse is still refused here - otherwise a malformed value would travel to
        /// the shell and fail somewhere with no name attached.
        pub fn from_str(v: &str) -> std::result::Result<HeaderValue, InvalidHeaderValue> {
            if v.bytes().any(|b| b < 0x20 && b != b'\t' || b == 0x7f) {
                return Err(InvalidHeaderValue {
                    name: "value",
                    value: v.to_owned(),
                });
            }
            Ok(HeaderValue(Cow::Owned(v.to_owned())))
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }

        pub fn to_str(&self) -> std::result::Result<&str, ToStrError> {
            if self.0.bytes().any(|b| b == 0 || b > 0x7f && b < 0x20) {
                return Err(ToStrError);
            }
            Ok(&self.0)
        }

        /// Recorded and ignored: `AUTHORIZATION` is marked sensitive so upstream will not echo it
        /// in debug output, and nothing here echoes headers.
        pub fn set_sensitive(&mut self, _sensitive: bool) {}
    }

    impl fmt::Display for HeaderValue {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    /// Anything usable as a header name at a `RequestBuilder` call site: the constants above and
    /// the string literals `phira` passes (`"Authorization"`, `"Content-Type"`).
    pub trait IntoHeaderName {
        fn into_header_name(self) -> HeaderName;
    }

    impl IntoHeaderName for HeaderName {
        fn into_header_name(self) -> HeaderName {
            self
        }
    }

    impl<'a> IntoHeaderName for &'a HeaderName {
        fn into_header_name(self) -> HeaderName {
            self.clone()
        }
    }

    /// Not restricted to `'static`: `RequestBuilder::header` is generic over `K`, so a local
    /// `&str` has to be accepted or the bound would silently only match literals.
    impl<'a> IntoHeaderName for &'a str {
        fn into_header_name(self) -> HeaderName {
            HeaderName(Cow::Owned(self.to_owned()))
        }
    }

    impl IntoHeaderName for String {
        fn into_header_name(self) -> HeaderName {
            HeaderName(Cow::Owned(self))
        }
    }

    /// Anything usable as a header value: `HeaderValue` itself, and the `String`/`&str` that
    /// `phira` builds inline (`.header("Authorization", format!("Bearer {token}"))`).
    pub trait IntoHeaderValue {
        fn into_header_value(self) -> HeaderValue;
    }

    impl IntoHeaderValue for HeaderValue {
        fn into_header_value(self) -> HeaderValue {
            self
        }
    }

    impl IntoHeaderValue for String {
        fn into_header_value(self) -> HeaderValue {
            HeaderValue(Cow::Owned(self))
        }
    }

    impl<'a> IntoHeaderValue for &'a HeaderValue {
        fn into_header_value(self) -> HeaderValue {
            self.clone()
        }
    }

    impl<'a> IntoHeaderValue for &'a str {
        fn into_header_value(self) -> HeaderValue {
            HeaderValue(Cow::Owned(self.to_owned()))
        }
    }

    /// Lookup key for [`HeaderMap::get`]. `&str` covers the only form `phira` uses
    /// (`headers.get("location")`); the `HeaderName` forms are there so a constant also works.
    pub trait AsHeaderName {
        fn as_header_name(&self) -> &str;
    }

    impl AsHeaderName for &str {
        fn as_header_name(&self) -> &str {
            self
        }
    }

    impl AsHeaderName for HeaderName {
        fn as_header_name(&self) -> &str {
            self.as_str()
        }
    }

    impl AsHeaderName for &HeaderName {
        fn as_header_name(&self) -> &str {
            self.as_str()
        }
    }

    /// A small ordered map. Order does not matter to the shell and a `Vec` keeps this honest about
    /// how few headers actually flow through it.
    #[derive(Clone, Debug, Default)]
    pub struct HeaderMap {
        entries: Vec<(HeaderName, HeaderValue)>,
    }

    impl HeaderMap {
        pub fn new() -> Self {
            HeaderMap {
                entries: Vec::new(),
            }
        }

        /// Appends rather than replaces, like upstream - `ACCEPT_LANGUAGE` is appended by
        /// `build_client`, and collapsing it to a single value would silently drop the first.
        pub fn append(&mut self, name: impl IntoHeaderName, value: impl IntoHeaderValue) {
            self.entries
                .push((name.into_header_name(), value.into_header_value()));
        }

        pub fn insert(&mut self, name: impl IntoHeaderName, value: impl IntoHeaderValue) {
            let name = name.into_header_name();
            let value = value.into_header_value();
            if let Some(slot) = self
                .entries
                .iter_mut()
                .find(|(n, _)| n.as_str().eq_ignore_ascii_case(name.as_str()))
            {
                slot.1 = value;
            } else {
                self.entries.push((name, value));
            }
        }

        /// Returns the *first* match, case-insensitively.
        pub fn get<K: AsHeaderName>(&self, key: K) -> Option<&HeaderValue> {
            let key = key.as_header_name();
            self.entries
                .iter()
                .find(|(n, _)| n.as_str().eq_ignore_ascii_case(key))
                .map(|(_, v)| v)
        }

        pub fn contains_key<K: AsHeaderName>(&self, key: K) -> bool {
            self.get(key).is_some()
        }

        pub fn len(&self) -> usize {
            self.entries.len()
        }

        pub fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
            self.entries.iter().map(|(k, v)| (k, v))
        }
    }

    /// `slice::Iter` yields `&(HeaderName, HeaderValue)`, while `&HeaderMap` has to yield the pair
    /// destructured - `for (k, v) in &headers` is how the header map is iterated everywhere. The
    /// adapter is named (rather than a closure) because `IntoIterator::IntoIter` must be spelled as
    /// a concrete type, and a closure's type cannot be written down.
    fn pair_ref(it: &(HeaderName, HeaderValue)) -> (&HeaderName, &HeaderValue) {
        let (k, v) = it;
        (k, v)
    }

    impl<'a> IntoIterator for &'a HeaderMap {
        type Item = (&'a HeaderName, &'a HeaderValue);
        type IntoIter = std::iter::Map<
            std::slice::Iter<'a, (HeaderName, HeaderValue)>,
            fn(&(HeaderName, HeaderValue)) -> (&HeaderName, &HeaderValue),
        >;
        fn into_iter(self) -> Self::IntoIter {
            self.entries.iter().map(pair_ref as fn(&(HeaderName, HeaderValue)) -> (&HeaderName, &HeaderValue))
        }
    }
}

// ---------------------------------------------------------------------------
// StatusCode / Method
// ---------------------------------------------------------------------------

/// HTTP status code.
///
/// A tuple struct with `PartialEq` rather than an enum, because `phira` matches on it:
/// `matches!(status, StatusCode::FORBIDDEN | StatusCode::CONFLICT)` only compiles for associated
/// constants of a structural-match type, not for enum variants named `StatusCode::FORBIDDEN`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusCode(u16);

impl StatusCode {
    pub const fn from_u16(code: u16) -> Self {
        StatusCode(code)
    }

    pub const fn as_u16(self) -> u16 {
        self.0
    }

    pub fn is_success(self) -> bool {
        (200..300).contains(&self.0)
    }

    pub fn is_redirection(self) -> bool {
        (300..400).contains(&self.0)
    }

    pub fn is_client_error(self) -> bool {
        (400..500).contains(&self.0)
    }

    pub fn is_server_error(self) -> bool {
        (500..600).contains(&self.0)
    }

    pub fn is_informational(self) -> bool {
        (100..200).contains(&self.0)
    }

    /// Canonical reason phrase, or a placeholder for a code outside the standard set.
    ///
    /// `phira` puts this straight into user-facing error text (`request failed (HTTP {status})`),
    /// so an unknown code has to produce something printable rather than an empty string that
    /// would read as a truncated message.
    pub fn as_str(self) -> &'static str {
        match self.0 {
            100 => "Continue",
            101 => "Switching Protocols",
            102 => "Processing",
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            203 => "Non Authoritative Information",
            204 => "No Content",
            205 => "Reset Content",
            206 => "Partial Content",
            207 => "Multi-Status",
            208 => "Already Reported",
            226 => "IM Used",
            300 => "Multiple Choices",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            304 => "Not Modified",
            305 => "Use Proxy",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            402 => "Payment Required",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            406 => "Not Acceptable",
            407 => "Proxy Authentication Required",
            408 => "Request Timeout",
            409 => "Conflict",
            410 => "Gone",
            411 => "Length Required",
            412 => "Precondition Failed",
            413 => "Payload Too Large",
            414 => "URI Too Long",
            415 => "Unsupported Media Type",
            416 => "Range Not Satisfiable",
            417 => "Expectation Failed",
            418 => "I'm a teapot",
            421 => "Misdirected Request",
            422 => "Unprocessable Entity",
            423 => "Locked",
            424 => "Failed Dependency",
            426 => "Upgrade Required",
            428 => "Precondition Required",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            451 => "Unavailable For Legal Reasons",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            505 => "HTTP Version Not Supported",
            506 => "Variant Also Negotiates",
            507 => "Insufficient Storage",
            508 => "Loop Detected",
            510 => "Not Extended",
            511 => "Network Authentication Required",
            _ => "Unknown Status Code",
        }
    }

    pub const OK: StatusCode = StatusCode(200);
    pub const CREATED: StatusCode = StatusCode(201);
    pub const NO_CONTENT: StatusCode = StatusCode(204);
    pub const MOVED_PERMANENTLY: StatusCode = StatusCode(301);
    pub const FOUND: StatusCode = StatusCode(302);
    pub const SEE_OTHER: StatusCode = StatusCode(303);
    pub const NOT_MODIFIED: StatusCode = StatusCode(304);
    pub const TEMPORARY_REDIRECT: StatusCode = StatusCode(307);
    pub const PERMANENT_REDIRECT: StatusCode = StatusCode(308);
    pub const BAD_REQUEST: StatusCode = StatusCode(400);
    pub const UNAUTHORIZED: StatusCode = StatusCode(401);
    pub const FORBIDDEN: StatusCode = StatusCode(403);
    pub const NOT_FOUND: StatusCode = StatusCode(404);
    pub const METHOD_NOT_ALLOWED: StatusCode = StatusCode(405);
    pub const REQUEST_TIMEOUT: StatusCode = StatusCode(408);
    pub const CONFLICT: StatusCode = StatusCode(409);
    pub const GONE: StatusCode = StatusCode(410);
    pub const LENGTH_REQUIRED: StatusCode = StatusCode(411);
    pub const PAYLOAD_TOO_LARGE: StatusCode = StatusCode(413);
    pub const URI_TOO_LONG: StatusCode = StatusCode(414);
    pub const UNSUPPORTED_MEDIA_TYPE: StatusCode = StatusCode(415);
    pub const RANGE_NOT_SATISFIABLE: StatusCode = StatusCode(416);
    pub const UNPROCESSABLE_ENTITY: StatusCode = StatusCode(422);
    pub const TOO_MANY_REQUESTS: StatusCode = StatusCode(429);
    pub const REQUEST_HEADER_FIELDS_TOO_LARGE: StatusCode = StatusCode(431);
    pub const INTERNAL_SERVER_ERROR: StatusCode = StatusCode(500);
    pub const NOT_IMPLEMENTED: StatusCode = StatusCode(501);
    pub const BAD_GATEWAY: StatusCode = StatusCode(502);
    pub const SERVICE_UNAVAILABLE: StatusCode = StatusCode(503);
    pub const GATEWAY_TIMEOUT: StatusCode = StatusCode(504);
    pub const HTTP_VERSION_NOT_SUPPORTED: StatusCode = StatusCode(505);
}

impl fmt::Debug for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.0, self.as_str())
    }
}

impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.0, self.as_str())
    }
}

/// Request method. Every value `phira` uses is a literal, so `&'static str` carries it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Method(&'static str);

impl Method {
    pub const GET: Method = Method("GET");
    pub const HEAD: Method = Method("HEAD");
    pub const POST: Method = Method("POST");
    pub const PUT: Method = Method("PUT");
    pub const PATCH: Method = Method("PATCH");
    pub const DELETE: Method = Method("DELETE");
    pub const OPTIONS: Method = Method("OPTIONS");

    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl fmt::Debug for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

// ---------------------------------------------------------------------------
// IntoUrl
// ---------------------------------------------------------------------------

/// Anything `Client::{get,post,...}` accepts.
///
/// `phira` passes all three shapes: owned `String` (`API_URL.to_string() + path`), borrowed
/// `&String` (`client.get(&self.url)` in `File::request`) and `url::Url` (`deeplink`'s target).
/// Without the `&String` impl the borrowed form fails to infer, which is a confusing error for a
/// one-line omission.
pub trait IntoUrl {
    fn into_url(self) -> std::result::Result<Url, Error>;
}

impl IntoUrl for Url {
    fn into_url(self) -> std::result::Result<Url, Error> {
        Ok(self)
    }
}

impl IntoUrl for &Url {
    fn into_url(self) -> std::result::Result<Url, Error> {
        Ok(self.clone())
    }
}

impl IntoUrl for String {
    fn into_url(self) -> std::result::Result<Url, Error> {
        Url::parse(&self).map_err(|e| Error::new(format!("invalid url {self:?}: {e}")))
    }
}

impl IntoUrl for &String {
    fn into_url(self) -> std::result::Result<Url, Error> {
        Url::parse(self).map_err(|e| Error::new(format!("invalid url {self:?}: {e}")))
    }
}

impl IntoUrl for &str {
    fn into_url(self) -> std::result::Result<Url, Error> {
        Url::parse(self).map_err(|e| Error::new(format!("invalid url {self:?}: {e}")))
    }
}

// ---------------------------------------------------------------------------
// redirect
// ---------------------------------------------------------------------------

pub mod redirect {
    use super::Url;

    /// The decision taken for one hop. `Attempt::follow`/`stop` return this, which is what makes
    /// `Policy::custom(|attempt| if .. { attempt.stop() } else { attempt.follow() })` compile.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Action {
        Follow,
        Stop,
    }

    /// One redirect, handed to the custom policy.
    #[derive(Debug)]
    pub struct Attempt {
        url: Url,
    }

    impl Attempt {
        pub(crate) fn new(url: Url) -> Self {
            Attempt { url }
        }

        pub fn url(&self) -> &Url {
            &self.url
        }

        pub fn stop(self) -> Action {
            Action::Stop
        }

        pub fn follow(self) -> Action {
            Action::Follow
        }
    }

    /// Redirect policy. Only `custom` and `none` are representable here, because those are the two
    /// forms in the workspace; `limited` exists so a future call site still compiles.
    pub struct Policy {
        custom: Option<Box<dyn FnMut(Attempt) -> Action + Send + Sync + 'static>>,
    }

    impl Policy {
        pub fn custom<F>(f: F) -> Policy
        where
            F: FnMut(Attempt) -> Action + Send + Sync + 'static,
        {
            Policy {
                custom: Some(Box::new(f)),
            }
        }

        pub fn none() -> Policy {
            Policy { custom: None }
        }

        pub fn limited(_max: u32) -> Policy {
            Policy { custom: None }
        }

        /// Runs the custom closure if one was installed. Not used by `send` yet - see the crate
        /// docs - but kept next to the storage so the redirect rule is one line to apply.
        pub fn decide(&mut self, url: Url) -> Action {
            match self.custom.as_mut() {
                Some(f) => f(Attempt::new(url)),
                None => Action::Follow,
            }
        }
    }

    impl Default for Policy {
        fn default() -> Self {
            Policy::none()
        }
    }
}

// ---------------------------------------------------------------------------
// Client / ClientBuilder
// ---------------------------------------------------------------------------

/// A configured client. Carries no connection pool: there is no transport behind it yet.
#[derive(Clone)]
pub struct Client {
    default_headers: header::HeaderMap,
    redirect: std::sync::Arc<std::sync::Mutex<redirect::Policy>>,
}

impl Client {
    pub fn new() -> Client {
        Client {
            default_headers: header::HeaderMap::new(),
            redirect: std::sync::Arc::new(std::sync::Mutex::new(redirect::Policy::none())),
        }
    }

    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let mut builder = RequestBuilder::new(method, url.into_url());
        for (k, v) in self.default_headers.iter() {
            builder = builder.header(k.clone(), v.clone());
        }
        builder
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }

    pub fn patch<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PATCH, url)
    }

    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

pub struct ClientBuilder {
    default_headers: header::HeaderMap,
    redirect: redirect::Policy,
    danger_accept_invalid_certs: bool,
}

impl ClientBuilder {
    pub fn new() -> ClientBuilder {
        ClientBuilder {
            default_headers: header::HeaderMap::new(),
            redirect: redirect::Policy::none(),
            danger_accept_invalid_certs: false,
        }
    }

    pub fn redirect(mut self, policy: redirect::Policy) -> ClientBuilder {
        self.redirect = policy;
        self
    }

    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> ClientBuilder {
        self.danger_accept_invalid_certs = accept;
        self
    }

    pub fn default_headers(mut self, headers: header::HeaderMap) -> ClientBuilder {
        self.default_headers = headers;
        self
    }

    /// Always succeeds: unlike upstream there is nothing here that can fail to construct, and
    /// `phira` propagates the error with `?` rather than unwrapping, so a permissive `Ok` keeps
    /// every call site on its normal path.
    pub fn build(self) -> std::result::Result<Client, Error> {
        Ok(Client {
            default_headers: self.default_headers,
            redirect: std::sync::Arc::new(std::sync::Mutex::new(self.redirect)),
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        ClientBuilder::new()
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// RequestBuilder
// ---------------------------------------------------------------------------

pub struct RequestBuilder {
    method: Method,
    url: std::result::Result<Url, Error>,
    headers: header::HeaderMap,
    body: Option<Vec<u8>>,
    query: Vec<(String, String)>,
    error: Option<Error>,
}

impl RequestBuilder {
    fn new(method: Method, url: std::result::Result<Url, Error>) -> Self {
        RequestBuilder {
            method,
            url,
            headers: header::HeaderMap::new(),
            body: None,
            query: Vec::new(),
            error: None,
        }
    }

    pub fn header<K, V>(mut self, key: K, value: V) -> RequestBuilder
    where
        K: header::IntoHeaderName,
        V: header::IntoHeaderValue,
    {
        self.headers.insert(key, value);
        self
    }

    pub fn bearer_auth(self, token: impl fmt::Display) -> RequestBuilder {
        self.header(
            header::AUTHORIZATION,
            format!("Bearer {}", token),
        )
    }

    /// Serializes `value` to JSON and uses it as the body, setting `Content-Type`.
    ///
    /// A serialization failure is recorded rather than returned: upstream also defers it to
    /// `send()`, and `phira`'s `Client::post` chains `.json(data)` directly onto the builder, so
    /// there is no `Result` to return it through.
    pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> RequestBuilder {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                self.headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                );
                self.body = Some(bytes);
            }
            Err(err) => {
                self.error = Some(Error::new(format!("failed to serialize request body: {err}")));
            }
        }
        self
    }

    /// Appends query parameters.
    ///
    /// Serialized through `serde_json` rather than `serde_urlencoded` so no extra dependency is
    /// needed, then flattened by shape: a map becomes `k=v` pairs and a sequence of 2-element
    /// arrays (how a slice of tuples serializes) becomes `k=v` pairs too. Those are the two forms
    /// `phira` passes - `HashMap<String, String>` and `&[("version", ".."), ("flavor", "..")]`.
    pub fn query<T: Serialize + ?Sized>(mut self, query: &T) -> RequestBuilder {
        match serde_json::to_value(query) {
            Ok(serde_json::Value::Object(map)) => {
                for (k, v) in map {
                    self.query.push((k, json_scalar_to_string(v)));
                }
            }
            Ok(serde_json::Value::Array(items)) => {
                for item in items {
                    if let serde_json::Value::Array(pair) = item {
                        if pair.len() == 2 {
                            let mut it = pair.into_iter();
                            let k = json_scalar_to_string(it.next().unwrap());
                            let v = json_scalar_to_string(it.next().unwrap());
                            self.query.push((k, v));
                        }
                    }
                }
            }
            Ok(_) | Err(_) => {
                self.error = Some(Error::new("query value is not an object or a list of pairs"));
            }
        }
        self
    }

    pub fn body<B: Into<Vec<u8>>>(mut self, body: B) -> RequestBuilder {
        self.body = Some(body.into());
        self
    }

    pub fn timeout<T>(mut self, _timeout: T) -> RequestBuilder {
        self
    }

    /// The actual transport call - see the crate docs for why this cannot do its job yet.
    pub async fn send(self) -> Result<Response> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let url = self.url?;
        let url_text = url.as_str().to_owned();
        Err(Error::transport(format!(
            "no HTTP transport for {url_text}: `reqwest` is shimmed on this build and the shell's \
             HTTP callback is not installed yet"
        )))
    }
}

impl fmt::Debug for RequestBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBuilder")
            .field("method", &self.method)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

fn json_scalar_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// A completed response.
///
/// Never constructed yet - every `send` fails first - but fully shaped, because the consumers
/// (`recv_raw`, `File::fetch`, `fetch_terms`) decode status, headers and body in ways that must
/// type-check for the crate to build.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: header::HeaderMap,
    body: Bytes,
    url: Url,
}

impl Response {
    pub fn new(status: StatusCode, headers: header::HeaderMap, body: Bytes, url: Url) -> Self {
        Response {
            status,
            headers,
            body,
            url,
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &header::HeaderMap {
        &self.headers
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn content_length(&self) -> Option<u64> {
        Some(self.body.len() as u64)
    }

    pub fn version(&self) -> u8 {
        1
    }

    pub async fn text(self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.body).into_owned())
    }

    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::new(format!("failed to decode response body: {e}")))
    }

    pub async fn bytes(self) -> Result<Bytes> {
        Ok(self.body)
    }

    /// The body as a stream of chunks, which is how `phira` downloads: `scene/song.rs` writes each
    /// chunk to disk as it arrives (so an 8 MB track is never buffered twice) and `deeplink.rs`
    /// counts bytes against a limit while downloading.
    ///
    /// Upstream the transport delivers this incrementally. Here the transport is the shell and it
    /// answers with a complete response, so the whole body is handed over as a single chunk - the
    /// two callers only do `write_all(&chunk)` and `count += chunk.len()`, which total the same
    /// either way, and neither relies on more than one chunk arriving.
    pub fn bytes_stream(self) -> BytesStream {
        BytesStream {
            body: Some(self.body),
        }
    }

    /// Fails when the status is a client or server error, matching upstream's contract that the
    /// `?` after it turns an HTTP error into an `Err`.
    pub fn error_for_status(self) -> Result<Response> {
        if self.status.is_client_error() || self.status.is_server_error() {
            return Err(Error {
                msg: format!("HTTP error status {}", self.status),
                status: Some(self.status),
                url: Some(self.url),
            });
        }
        Ok(self)
    }
}

/// See [`Response::bytes_stream`].
///
/// Yields the buffered body once and then ends: `body` is `Some` for exactly one `poll_next`.
/// Written out by hand rather than with `futures_util::stream::once`, because `futures-util` is a
/// much larger dependency than the `futures-core` trait this has to satisfy, and the shim should
/// not decide what else goes into the graph.
pub struct BytesStream {
    body: Option<Bytes>,
}

impl futures_core::Stream for BytesStream {
    type Item = Result<Bytes>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // `BytesStream` is `Unpin` (a single `Option<Bytes>`), so `Pin<&mut Self>` derefs mutably
        // and `take` is available without a projection.
        std::task::Poll::Ready(match self.body.take() {
            Some(body) if !body.is_empty() => Some(Ok(body)),
            _ => None,
        })
    }
}

/// `reqwest::get(url)` - the free function `censor` and `phira-monitor` use.
pub async fn get<U: IntoUrl>(url: U) -> Result<Response> {
    Client::new().get(url).send().await
}

/// `reqwest::post(url)` exists upstream; kept so a call site written against it still builds.
pub async fn post<U: IntoUrl>(url: U) -> Result<Response> {
    Client::new().post(url).send().await
}
