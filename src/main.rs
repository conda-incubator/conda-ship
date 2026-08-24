//! Generic single-binary conda bootstrap runtime powered by rattler.

use std::env;

use miette::IntoDiagnostic;

mod bootstrap_lock;
mod bootstrap_state;
mod commands;
mod config;
mod constructor_metadata;
mod exec;
mod executable_update;
mod hash;
mod http;
mod install;
mod policy;
mod runtime_data;
mod runtime_update;
mod tls;

use commands::ensure_bootstrapped;

#[used]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__TEXT,__cship_reader"))]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".cscap"))]
static READER_CAPABILITY_RECORD: runtime_data::ReaderCapabilityRecord =
    runtime_data::reader_capability_record();

fn main() -> miette::Result<()> {
    // Keep the template-only ABI declaration reachable as well as linker-retained.
    std::hint::black_box(&READER_CAPABILITY_RECORD);
    tls::install_default_provider();

    let num_cores = std::thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .max(2);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cores / 2)
        .max_blocking_threads(num_cores)
        .enable_all()
        .build()
        .into_diagnostic()?;

    runtime.block_on(async_main())
}

async fn async_main() -> miette::Result<()> {
    init_tracing()?;
    ensure_stamped_runtime()?;

    let prefix = policy::install_path()?;
    if let Some(action) = env::var_os(runtime_update::INTERNAL_UPDATE_ENV) {
        let action = action
            .to_str()
            .ok_or_else(|| miette::miette!("invalid internal runtime update action"))?;
        if action == runtime_update::RECORD_INSTALLATION_ACTION {
            ensure_bootstrapped(&prefix).await?;
        }
        return runtime_update::run_internal_helper(action, &prefix).await;
    }
    ensure_bootstrapped(&prefix).await?;
    if !runtime_update::recover_pending(&prefix)? {
        runtime_update::initialize_current(&prefix)?;
    }

    let args: Vec<_> = env::args_os().skip(1).collect();
    exec::replace_process_with_delegate(&prefix, policy::delegate_executable(), &args)
}

fn ensure_stamped_runtime() -> miette::Result<()> {
    if runtime_data::current().stamped
        || env::var_os("CONDA_SHIP_ALLOW_UNSTAMPED_TEMPLATE").is_some()
    {
        return Ok(());
    }

    Err(miette::miette!(
        "{} is a runtime template, not a runnable conda runtime. Build a stamped runtime with `cs build`.",
        policy::display_name(),
    ))
}

fn init_tracing() -> miette::Result<()> {
    use tracing_subscriber::{EnvFilter, filter::LevelFilter, util::SubscriberInitExt};

    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env()
        .into_diagnostic()?;

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .without_time()
        .finish()
        .try_init()
        .into_diagnostic()?;

    Ok(())
}
