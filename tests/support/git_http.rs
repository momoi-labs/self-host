use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Git's CGI backend exposes only the fresh synthetic repository.
pub async fn git_http(root: PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let root = root.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let end = loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break index + 4;
                    }
                    assert!(request.len() < 65536, "fixture request header too large");
                };
                let header = String::from_utf8_lossy(&request[..end]).into_owned();
                let first: Vec<_> = header.lines().next().unwrap().split_whitespace().collect();
                let (path, query) = first[1].split_once('?').unwrap_or((first[1], ""));
                let field = |name: &str| {
                    header
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim())
                        .unwrap_or("")
                        .to_owned()
                };
                let length: usize = field("content-length").parse().unwrap_or_default();
                assert!(length < 1048576, "fixture request body too large");
                while request.len() - end < length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let mut child = Command::new("git")
                    .arg("http-backend")
                    .env("GIT_PROJECT_ROOT", root)
                    .env("GIT_HTTP_EXPORT_ALL", "1")
                    .env("PATH_INFO", path)
                    .env("QUERY_STRING", query)
                    .env("REQUEST_METHOD", first[0])
                    .env("CONTENT_TYPE", field("content-type"))
                    .env("CONTENT_LENGTH", length.to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&request[end..])
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                let split = output
                    .stdout
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let headers = String::from_utf8_lossy(&output.stdout[..split]);
                let status = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Status: "))
                    .unwrap_or("200 OK");
                let headers = headers
                    .lines()
                    .filter(|line| !line.starts_with("Status:"))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                let response = format!(
                    "HTTP/1.1 {status}\r\n{headers}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    output.stdout.len() - split - 4
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&output.stdout[split + 4..]).await.unwrap();
            });
        }
    });
    (format!("http://{address}/repo/.git"), server)
}
