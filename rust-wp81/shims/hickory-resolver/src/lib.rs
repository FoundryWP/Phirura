//! A `hickory-resolver` replacement for Windows Phone 8.1.
//!
//! # Why
//!
//! `phira-mp-client` (multiplayer, a plain non-optional dependency of `phira`) resolves the
//! `_phira._tcp` SRV record through `hickory-resolver`, whose default features pull
//! `hickory-proto -> ring`. `ring` is a C/assembly crypto library whose build script fails here
//! for the same reason `aws-lc-sys` does: there is no toolchain path from a `cl.exe` that only
//! knows x86 and ARM to a `thumbv7a-pc-windows-msvc` target. Shimming this crate removes that
//! whole subtree rather than teaching `ring` to build.
//!
//! # Why this and not `phira-mp-client`
//!
//! Only one file in `phira-mp-client` mentions hickory - `resolver.rs`, five types and one
//! lookup. Replacing *it* leaves `phira-mp-client`'s real API intact, so `phira::scene::song` and
//! `phira::mp::panel` keep compiling against the genuine `Client`, `RoomId`, `ClientCommand` and
//! friends. Shim `phira-mp-client` instead and all of that has to be re-declared from guesswork.
//!
//! # What is not implemented
//!
//! The lookup. There is no DNS client here, and none is wanted: multiplayer is out of scope for
//! this port, so `srv_lookup` answers with an error and the connect path reports it the same way
//! it would report a missing record.

#![allow(unused_variables)]

use std::fmt;

/// Resolution failure.
///
/// Only `Display` is required by the single call site (`anyhow!("SRV lookup failed: {}", e)`), but
/// `std::error::Error` is implemented as well so a `?` into `anyhow` would work if a future call
/// site propagates instead of formatting.
#[derive(Debug)]
pub struct ResolveError {
    msg: String,
}

impl ResolveError {
    fn new(msg: impl Into<String>) -> Self {
        ResolveError { msg: msg.into() }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for ResolveError {}

/// Stand-in for the upstream builder options. Carries no fields because nothing reads them.
#[derive(Clone, Debug, Default)]
pub struct ResolverOpts;

/// Stand-in for the upstream server list. `phira-mp-client` only constructs the default.
#[derive(Clone, Debug, Default)]
pub struct ResolverConfig;

/// The connection provider, which selects how upstream talks to a name server. Unit type here:
/// there is no connection to provide.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioConnectionProvider;

/// One SRV record target, as `resolver.rs` reads it: `srv.target().to_string()` and `srv.port()`.
///
/// Exposed as *methods* because the compiler type-checks `resolver.rs` even though `srv_lookup`
/// always fails and the line is never reached at runtime - fields alone would not satisfy it.
#[derive(Clone, Debug)]
pub struct Srv {
    target: String,
    port: u16,
}

impl Srv {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// The (empty) result of a lookup.
///
/// Never constructed - `srv_lookup` always fails - but shaped so `lookup.iter().next()` and the
/// field reads after it type-check.
#[derive(Debug, Default)]
pub struct Lookup {
    records: Vec<Srv>,
}

impl Lookup {
    pub fn iter(&self) -> std::slice::Iter<'_, Srv> {
        self.records.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

impl IntoIterator for Lookup {
    type Item = Srv;
    type IntoIter = std::vec::IntoIter<Srv>;
    fn into_iter(self) -> Self::IntoIter {
        self.records.into_iter()
    }
}

/// Half of the builder pair `resolver.rs` uses:
/// `TokioResolver::builder_with_config(..).with_options(..).build()`.
pub struct Builder {
    options: ResolverOpts,
}

/// The resolver itself. A unit type, which is what lets `static RESOLVER: LazyLock<TokioResolver>`
/// be `Sync` without a phantom payload.
#[derive(Debug, Default)]
pub struct TokioResolver;

impl TokioResolver {
    pub fn builder_with_config(
        _config: ResolverConfig,
        _provider: TokioConnectionProvider,
    ) -> Builder {
        Builder {
            options: ResolverOpts::default(),
        }
    }

    /// The only lookup `resolver.rs` performs. Answers with an error: this build has no DNS
    /// client, and multiplayer is not part of the port.
    pub async fn srv_lookup(&self, name: &str) -> Result<Lookup, ResolveError> {
        Err(ResolveError::new(format!(
            "DNS lookup for {name} is unavailable: `hickory-resolver` is shimmed on this build"
        )))
    }

    /// Kept so a future `ip_lookup` call site compiles rather than forcing another rebuild.
    pub async fn ip_lookup(&self, name: &str) -> Result<Lookup, ResolveError> {
        self.srv_lookup(name).await
    }
}

impl Builder {
    /// Recorded and ignored; `ResolverOpts` is empty, so there is nothing to apply it to.
    pub fn with_options(mut self, options: ResolverOpts) -> Builder {
        self.options = options;
        self
    }

    pub fn build(self) -> TokioResolver {
        // Read, then dropped: the options are accepted so the builder chain compiles, and there is
        // no resolver behind it for them to configure.
        let _ = self.options;
        TokioResolver
    }
}

pub mod config {
    pub use super::{ResolverConfig, ResolverOpts};
}

pub mod name_server {
    pub use super::TokioConnectionProvider;
}
