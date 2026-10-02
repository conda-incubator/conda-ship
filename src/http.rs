use std::sync::Arc;
use std::time::Duration;

use miette::{Context, IntoDiagnostic};
use rattler_networking::AuthenticationMiddleware;

pub(crate) const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[allow(dead_code)]
pub(crate) fn download_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(false, false)
}

#[allow(dead_code)]
pub(crate) fn runtime_update_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(true, false)
}

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[allow(dead_code)]
pub(crate) fn runtime_update_probe_client()
-> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(true, true)
}

fn make_download_client(
    reject_https_downgrade: bool,
    probe: bool,
) -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    crate::tls::install_default_provider();

    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .no_gzip()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600));
    let builder = if probe {
        builder
            .connect_timeout(PROBE_TIMEOUT)
            .timeout(PROBE_TIMEOUT)
            .retry(reqwest::retry::never())
    } else {
        builder
    };
    let builder = if reject_https_downgrade {
        builder.redirect(redirect_policy())
    } else {
        builder
    };
    let raw = builder
        .build()
        .into_diagnostic()
        .context("failed to create HTTP client")?;

    Ok(reqwest_middleware::ClientBuilder::new(raw.clone())
        .with_arc(Arc::new(
            AuthenticationMiddleware::from_env_and_defaults().into_diagnostic()?,
        ))
        .with(rattler_networking::OciMiddleware::new(raw))
        .build())
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt
            .previous()
            .last()
            .is_some_and(|previous| is_https_downgrade(previous, attempt.url()))
        {
            attempt.error("refusing an HTTPS redirect to a non-HTTPS URL")
        } else if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

fn is_https_downgrade(previous: &reqwest::Url, next: &reqwest::Url) -> bool {
    previous.scheme() == "https" && next.scheme() != "https"
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use super::{
        USER_AGENT, is_https_downgrade, runtime_update_client, runtime_update_probe_client,
    };

    #[test]
    fn user_agent_names_conda_ship() {
        assert!(USER_AGENT.starts_with("conda-ship/"));
    }

    #[test]
    fn redirects_never_downgrade_https() {
        let secure = reqwest::Url::parse("https://packages.example.test/runtime").unwrap();
        let other_secure = reqwest::Url::parse("https://cdn.example.test/runtime").unwrap();
        let insecure = reqwest::Url::parse("http://cdn.example.test/runtime").unwrap();

        assert!(!is_https_downgrade(&secure, &other_secure));
        assert!(is_https_downgrade(&secure, &insecure));
    }

    #[rstest::rstest]
    #[case(runtime_update_client)]
    #[case(runtime_update_probe_client)]
    #[tokio::test]
    async fn auth_file_credentials_are_applied_to_runtime_updates(
        #[case] make_client: fn() -> miette::Result<reqwest_middleware::ClientWithMiddleware>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let auth_file = temp.path().join("auth.json");
        std::fs::write(
            &auth_file,
            r#"{"127.0.0.1":{"BearerToken":"private-token"}}"#,
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = vec![0; 8192];
            let length = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            String::from_utf8(request[..length].to_vec()).unwrap()
        });
        let client = temp_env::with_var(
            "RATTLER_AUTH_FILE",
            Some(auth_file.as_os_str()),
            make_client,
        )
        .unwrap();

        let response = client
            .get(format!("http://{address}/repodata.json"))
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success());
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("authorization: bearer private-token\r\n"));
    }
}
