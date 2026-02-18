const MAINTENANCE_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

pub(crate) async fn wait_for_maintenance_shutdown(
    maintenance_task: tokio::task::JoinHandle<()>,
) -> Result<(), anyhow::Error> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(MAINTENANCE_SHUTDOWN_TIMEOUT_SECS),
        maintenance_task,
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(anyhow::anyhow!("maintenance task join failed: {err}")),
        Err(_) => Err(anyhow::anyhow!(
            "maintenance task did not shut down within {} seconds",
            MAINTENANCE_SHUTDOWN_TIMEOUT_SECS
        )),
    }
}

pub(crate) async fn drain_session_tasks(
    session_tasks: &mut tokio::task::JoinSet<()>,
    timeout: std::time::Duration,
) {
    let drain = async {
        while let Some(joined) = session_tasks.join_next().await {
            log_session_task_join_result(joined);
        }
    };

    if tokio::time::timeout(timeout, drain).await.is_ok() {
        return;
    }

    eprintln!(
        "session task drain timed out after {} seconds; aborting remaining tasks",
        timeout.as_secs()
    );
    session_tasks.abort_all();
    while let Some(joined) = session_tasks.join_next().await {
        log_session_task_join_result(joined);
    }
}

pub(crate) fn log_session_task_join_result(joined: Result<(), tokio::task::JoinError>) {
    match joined {
        Ok(()) => {}
        Err(err) if err.is_cancelled() => {
            eprintln!("session task cancelled during shutdown");
        }
        Err(err) => {
            eprintln!("session task join failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::drain_session_tasks;
    use std::time::Duration;
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn drain_session_tasks_completes_finished_tasks() {
        let mut session_tasks = JoinSet::new();
        session_tasks.spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        });
        session_tasks.spawn(async {});

        drain_session_tasks(&mut session_tasks, Duration::from_secs(1)).await;
        assert!(
            session_tasks.is_empty(),
            "all finished tasks should be drained"
        );
    }

    #[tokio::test]
    async fn drain_session_tasks_aborts_stuck_tasks_after_timeout() {
        let mut session_tasks = JoinSet::new();
        session_tasks.spawn(async {
            std::future::pending::<()>().await;
        });

        tokio::time::timeout(
            Duration::from_secs(1),
            drain_session_tasks(&mut session_tasks, Duration::from_millis(20)),
        )
        .await
        .expect("drain should finish by aborting stuck tasks");

        assert!(
            session_tasks.is_empty(),
            "stuck tasks should be aborted and drained"
        );
    }
}
