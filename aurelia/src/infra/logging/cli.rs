use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

pub fn init_cli_logging(verbosity: u8) {
    super::relay::set_local_verbosity(verbosity);
    let filter = resolve_filter(verbosity);

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(verbosity >= 2)
        .log_internal_errors(false)
        .init();
}

pub fn init_daemon_logging(verbosity: u8) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let own = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(verbosity >= 2)
        .log_internal_errors(false)
        .with_filter(resolve_filter(verbosity));

    tracing_subscriber::registry()
        .with(own)
        .with(relay_layers())
        .with(daemon_watch_layers())
        .init();
}

const MAX_VERBOSITY: u8 = 3;

fn relay_layers<S>() -> Vec<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer;

    (0..=MAX_VERBOSITY)
        .map(|level| {
            tracing_subscriber::fmt::layer()
                .with_writer(super::relay::RelayWriter)
                .with_ansi(false)
                .with_target(level >= 2)
                .log_internal_errors(false)
                .with_filter(RelayFilter {
                    level,
                    directives: EnvFilter::new(default_directives(level)),
                    scope: RelayScope::Request,
                })
                .boxed()
        })
        .collect()
}

fn daemon_watch_layers<S>() -> Vec<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer;

    (0..=MAX_VERBOSITY)
        .map(|level| {
            tracing_subscriber::fmt::layer()
                .with_writer(super::relay::DaemonWatchWriter {
                    level,
                    max: MAX_VERBOSITY,
                })
                .with_ansi(false)
                .with_target(level >= 2)
                .log_internal_errors(false)
                .with_filter(RelayFilter {
                    level,
                    directives: EnvFilter::new(default_directives(level)),
                    scope: RelayScope::Daemon,
                })
                .boxed()
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RelayScope {
    Request,
    Daemon,
}

struct RelayFilter {
    level: u8,
    directives: EnvFilter,
    scope: RelayScope,
}

impl<S> tracing_subscriber::layer::Filter<S> for RelayFilter {
    fn enabled(
        &self,
        meta: &tracing::Metadata<'_>,
        cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let in_scope = match (self.scope, super::relay::current_verbosity()) {
            (RelayScope::Request, Some(v)) => v.min(MAX_VERBOSITY) == self.level,
            (RelayScope::Daemon, None) => {
                super::relay::has_daemon_watcher(self.level, MAX_VERBOSITY)
            }
            _ => false,
        };
        in_scope
            && <EnvFilter as tracing_subscriber::layer::Filter<S>>::enabled(
                &self.directives,
                meta,
                cx,
            )
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        None
    }
}

fn resolve_filter(verbosity: u8) -> EnvFilter {
    EnvFilter::try_from_env("AURELIA_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(default_directives(verbosity)))
}

fn default_directives(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "debug,steam_vent=info,steam_vent_proto=info",
        2 => "debug,steam_vent=debug,steam_vent_proto=debug",
        _ => "trace,steam_vent=debug",
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
