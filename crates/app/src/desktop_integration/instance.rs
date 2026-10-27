use super::DesktopEvent;
use fs2::FileExt;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

/// Holds the process lock and the local activation listener until app shutdown.
pub struct Instance {
    _lock: File,
    stop: Option<oneshot::Sender<()>>,
    #[cfg(unix)]
    socket: PathBuf,
}

impl Instance {
    /// `None` means the running instance acknowledged a request to show its
    /// main window. Failure to wake it is an error, never permission to start
    /// a second owner of the headphones.
    pub async fn acquire(
        directory: &Path,
        events: mpsc::Sender<DesktopEvent>,
    ) -> Result<Option<Self>, String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("desktop.lock"))
            .map_err(|e| e.to_string())?;
        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wake_existing(directory).await?;
                return Ok(None);
            }
            Err(error) => return Err(format!("Unable to lock the application instance: {error}")),
        }
        let (stop, stopped) = oneshot::channel();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let socket = directory.join("desktop.sock");
            // The process lock proves this is a stale endpoint of this app.
            match std::fs::remove_file(&socket) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            let listener = tokio::net::UnixListener::bind(&socket).map_err(|error| {
                format!("Unable to create the local activation socket: {error}")
            })?;
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
            tokio::spawn(listen_unix(listener, stopped, events));
            Ok(Some(Self {
                _lock: lock,
                stop: Some(stop),
                socket,
            }))
        }
        #[cfg(windows)]
        {
            let name = pipe_name(directory);
            let server = tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
                .map_err(|error| format!("Unable to create the local activation pipe: {error}"))?;
            tokio::spawn(listen_windows(server, name, stopped, events));
            Ok(Some(Self {
                _lock: lock,
                stop: Some(stop),
            }))
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket);
        // Keep the lock file itself: unlinking it permits two independent locks.
    }
}

async fn activation<T: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut T,
    events: &mpsc::Sender<DesktopEvent>,
) {
    let mut request = [0; 5];
    if stream.read_exact(&mut request).await.is_ok()
        && request == *b"show\n"
        && events.send(DesktopEvent::OpenMain).await.is_ok()
    {
        let _ = stream.write_all(b"ok").await;
    }
}

async fn wake<T: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut T) -> Result<(), String> {
    stream
        .write_all(b"show\n")
        .await
        .map_err(|e| e.to_string())?;
    let mut ack = [0; 2];
    stream
        .read_exact(&mut ack)
        .await
        .map_err(|e| e.to_string())?;
    if ack == *b"ok" {
        Ok(())
    } else {
        Err("The running instance did not acknowledge activation".into())
    }
}

#[cfg(unix)]
async fn listen_unix(
    listener: tokio::net::UnixListener,
    mut stopped: oneshot::Receiver<()>,
    events: mpsc::Sender<DesktopEvent>,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => {
                let Ok((mut stream, _)) = accepted else { break; };
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = tokio::time::timeout(Duration::from_secs(2), activation(&mut stream, &events)) => {}
                }
            }
        }
    }
}

#[cfg(unix)]
async fn wake_existing(directory: &Path) -> Result<(), String> {
    for _ in 0..20 {
        match tokio::net::UnixStream::connect(directory.join("desktop.sock")).await {
            Ok(mut stream) => {
                return tokio::time::timeout(Duration::from_secs(2), wake(&mut stream))
                    .await
                    .map_err(|_| "The running instance did not respond".to_owned())?;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    Err("Another Easy Sound Connect instance owns the session but cannot be activated".into())
}

#[cfg(windows)]
fn pipe_name(directory: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    directory.hash(&mut hash);
    format!(r"\\.\pipe\ezsc-{:016x}", hash.finish())
}

#[cfg(windows)]
async fn listen_windows(
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    name: String,
    mut stopped: oneshot::Receiver<()>,
    events: mpsc::Sender<DesktopEvent>,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            connected = server.connect() => {
                if connected.is_err() { break; }
                // Keep an acceptor alive so the named pipe never disappears.
                let Ok(next) = tokio::net::windows::named_pipe::ServerOptions::new()
                    .reject_remote_clients(true).create(&name) else { break; };
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = tokio::time::timeout(Duration::from_secs(2), activation(&mut server, &events)) => {}
                }
                server = next;
            }
        }
    }
}

#[cfg(windows)]
async fn wake_existing(directory: &Path) -> Result<(), String> {
    for _ in 0..20 {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name(directory)) {
            Ok(mut stream) => {
                return tokio::time::timeout(Duration::from_secs(2), wake(&mut stream))
                    .await
                    .map_err(|_| "The running instance did not respond".to_owned())?;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    Err("Another Easy Sound Connect instance owns the session but cannot be activated".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_instance_activates_the_first_and_release_allows_restart() {
        let directory =
            std::env::temp_dir().join(format!("ezsc-instance-test-{}", std::process::id()));
        let (tx, mut rx) = mpsc::channel(4);
        let first = Instance::acquire(&directory, tx.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(
            Instance::acquire(&directory, tx.clone())
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(rx.recv().await, Some(DesktopEvent::OpenMain)));
        drop(first);
        let next = Instance::acquire(&directory, tx).await.unwrap().unwrap();
        drop(next);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
