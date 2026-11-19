use anyhow::Result;
use clap::Parser;
use std::{ffi::OsString, time::Duration};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const NAME: &str = "P2PTunnelGateway";
define_windows_service!(service_entry, service_main);

pub fn dispatch() -> Result<()> {
    Ok(service_dispatcher::start(NAME, service_entry)?)
}

fn service_main(_: Vec<OsString>) {
    if let Err(e) = execute() {
        eprintln!("gateway service: {e:#}");
    }
}

fn execute() -> Result<()> {
    let (sender, mut stop) = tokio::sync::watch::channel(false);
    let handle = service_control_handler::register(NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            sender.send_replace(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let report = |state: ServiceState, code: u32| {
        handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: u32::from(matches!(
                state,
                ServiceState::StartPending | ServiceState::StopPending
            )),
            wait_hint: if matches!(
                state,
                ServiceState::StartPending | ServiceState::StopPending
            ) {
                Duration::from_secs(20)
            } else {
                Duration::ZERO
            },
            process_id: None,
        })
    };
    report(ServiceState::StartPending, 0)?;
    let result = tokio::runtime::Runtime::new()
        .map_err(anyhow::Error::from)
        .and_then(|rt| {
            rt.block_on(super::run(
                super::Options::parse(),
                async {
                    stop.wait_for(|stopped| *stopped).await?;
                    report(ServiceState::StopPending, 0)?;
                    Ok(())
                },
                || Ok(report(ServiceState::Running, 0)?),
            ))
        });
    report(ServiceState::Stopped, u32::from(result.is_err()))?;
    result
}
