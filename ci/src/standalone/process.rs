//! A tutorial terminal: a long-running command stopped when dropped.
use anyhow::{bail, Context, Result};
use std::{
    fs::File,
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct Process {
    child: Child,
    log: std::path::PathBuf,
}

impl Process {
    pub fn start(args: &[String], cwd: &Path, env: &crate::command::Environment) -> Result<Self> {
        let log = cwd.join(format!("{}.log", args[0]));
        let child = Command::new(&args[0])
            .args(&args[1..])
            .current_dir(cwd)
            .env_remove("DATABASE_URL")
            .env_remove("BASED_DATABASE_URL")
            .envs(env)
            .stdin(Stdio::null())
            .stdout(File::create(&log)?)
            .stderr(File::create(cwd.join(format!("{}.err.log", args[0])))?)
            .spawn()
            .with_context(|| format!("start {args:?}"))?;
        Ok(Self { child, log })
    }

    /// Wait until `port` accepts connections, failing early if the process exits.
    pub fn listening(&mut self, port: u16) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                let errors =
                    std::fs::read_to_string(self.log.with_extension("err.log")).unwrap_or_default();
                bail!("{} exited with {status}:\n{errors}", self.log.display());
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("{} did not listen on {port}", self.log.display())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}
