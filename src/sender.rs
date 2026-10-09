use std::{
    fmt,
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug)]
pub struct SendError(String);

impl fmt::Display for SendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SendError {}

pub fn send(command: &[String], message: &[u8]) -> Result<(), SendError> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| SendError("Configure send_command before sending.".into()))?;
    if program == "termail-test-sink" {
        let path = arguments
            .first()
            .ok_or_else(|| SendError("termail-test-sink needs an output path.".into()))?;
        return write_test_sink(Path::new(path), message)
            .map_err(|error| SendError(error.to_string()));
    }

    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| SendError(error.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| SendError("Unable to open sender input.".into()))?
        .write_all(message)
        .map_err(|error| SendError(error.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|error| SendError(error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(SendError(if error.is_empty() {
            format!("Sender exited with {}.", output.status)
        } else {
            error
        }))
    }
}

fn write_test_sink(path: &Path, message: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(message)?;
    file.write_all(b"\n\n--- termail message ---\n")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::send;

    #[test]
    fn writes_messages_to_test_sink() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("termail-sink-{nonce}.eml"));
        send(
            &["termail-test-sink".into(), path.display().to_string()],
            b"Subject: hello\n\nbody",
        )
        .unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert!(contents.contains("Subject: hello"));
    }
}
