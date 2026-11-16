use anyhow::Result;
use std::{
    fmt,
    io::{self, Write},
};

#[derive(Debug)]
struct WriteError(io::Error);

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Could not write command output")
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn write_line(writer: &mut impl Write, arguments: fmt::Arguments<'_>) -> Result<()> {
    writeln!(writer, "{arguments}").map_err(WriteError)?;
    writer.flush().map_err(WriteError)?;
    Ok(())
}

pub fn line(arguments: fmt::Arguments<'_>) -> Result<()> {
    write_line(&mut io::stdout().lock(), arguments)
}

pub fn handle_closed_pipe(result: Result<()>) -> Result<()> {
    match result {
        Err(error)
            if error
                .downcast_ref::<WriteError>()
                .is_some_and(|write| write.0.kind() == io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failure {
        kind: io::ErrorKind,
        on_flush: bool,
    }

    impl Write for Failure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.on_flush {
                Ok(bytes.len())
            } else {
                Err(io::Error::new(self.kind, "Output probe"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.on_flush {
                Err(io::Error::new(self.kind, "Output probe"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn output_preserves_formatting_and_one_final_newline() {
        let mut bytes = Vec::new();
        write_line(&mut bytes, format_args!("{}", "[\n  1\n]")).unwrap();
        write_line(&mut bytes, format_args!("{{\"id\":{}}}", 2)).unwrap();
        assert_eq!(bytes, b"[\n  1\n]\n{\"id\":2}\n");
    }

    #[test]
    fn broken_pipe_from_a_write_or_flush_is_a_quiet_stop() {
        for on_flush in [false, true] {
            let result = write_line(
                &mut Failure {
                    kind: io::ErrorKind::BrokenPipe,
                    on_flush,
                },
                format_args!("Probe"),
            );
            assert!(result.is_err());
            handle_closed_pipe(result).unwrap();
        }
    }

    #[test]
    fn other_output_failures_remain_actionable_errors() {
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            for on_flush in [false, true] {
                let result = write_line(&mut Failure { kind, on_flush }, format_args!("Probe"));
                let error = handle_closed_pipe(result).unwrap_err();
                assert_eq!(error.to_string(), "Could not write command output");
                assert!(format!("{error:#}").contains("Output probe"));
            }
        }
    }

    #[test]
    fn unrelated_broken_pipe_errors_are_not_suppressed() {
        let result = Err(io::Error::new(io::ErrorKind::BrokenPipe, "Unrelated operation").into());
        let error = handle_closed_pipe(result).unwrap_err();
        assert_eq!(error.to_string(), "Unrelated operation");
    }
}
