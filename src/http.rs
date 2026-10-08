use std::sync::Arc;
use std::time::Duration;

use miette::{Context, IntoDiagnostic};
use rattler_networking::AuthenticationMiddleware;

pub(crate) const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub(crate) fn validate_artifact_url(value: &str, label: &str) -> miette::Result<()> {
    // Channels may be names or protocol-relative URLs. Parse both to check userinfo.
    let base = reqwest::Url::parse("https://channel.invalid/").expect("valid base URL");
    let url = reqwest::Url::options()
        .base_url(Some(&base))
        .parse(value)
        .into_diagnostic()
        .with_context(|| format!("invalid {label}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(miette::miette!("{label} must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(miette::miette!(
            "{label} must not contain a query or fragment"
        ));
    }

    // Check the text that is published, before URL parsing removes dot segments.
    let mut path = value.as_bytes().to_vec();
    loop {
        let mut previous = &[][..];
        for segment in path.split(|byte| matches!(byte, b'/' | b'\\')) {
            if previous == b"t" && !segment.is_empty() {
                return Err(miette::miette!("{label} must not contain credentials"));
            }
            previous = segment;
        }
        // Proxies can decode paths more than once, including encoded separators.
        let decoded = percent_encoding::percent_decode(&path).collect::<Vec<_>>();
        if decoded == path {
            return Ok(());
        }
        path = decoded;
    }
}

#[allow(dead_code)]
pub(crate) fn download_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(false)
}

#[allow(dead_code)]
pub(crate) fn runtime_update_client() -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(false)
}

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[allow(dead_code)]
pub(crate) fn runtime_update_probe_client()
-> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    make_download_client(true)
}

fn make_download_client(probe: bool) -> miette::Result<reqwest_middleware::ClientWithMiddleware> {
    crate::tls::install_default_provider();

    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(redirect_policy())
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
        USER_AGENT, download_client, is_https_downgrade, runtime_update_client,
        runtime_update_probe_client, validate_artifact_url,
    };

    #[rstest::rstest]
    #[case::username("https://private-token@example.test/channel", "credentials")]
    #[case::password("https://user:private-token@example.test/channel", "credentials")]
    #[case::query(
        "https://example.test/channel?token=private-token",
        "query or fragment"
    )]
    #[case::fragment("https://example.test/channel#private-token", "query or fragment")]
    #[case::token("https://example.test/t/private-token/channel", "credentials")]
    #[case::nested_token(
        "https://example.test/channel/t/private-token/pkg.conda",
        "credentials"
    )]
    #[case::encoded_t("https://example.test/%74/private-token/channel", "credentials")]
    #[case::encoded_separator(
        "https://example.test/channel%2ft%2fprivate-token/pkg.conda",
        "credentials"
    )]
    #[case::encoded_uppercase_separator(
        "https://example.test/channel%2Ft%2Fprivate-token/pkg.conda",
        "credentials"
    )]
    #[case::encoded_backslash(
        "https://example.test/channel%5ct%5cprivate-token/pkg.conda",
        "credentials"
    )]
    #[case::double_encoded(
        "https://example.test/channel%252f%2574%252fprivate-token/pkg.conda",
        "credentials"
    )]
    #[case::triple_encoded(
        "https://example.test/channel%25252ft%25252fprivate-token/pkg.conda",
        "credentials"
    )]
    #[case::file_token("file:///channel/t/private-token/pkg.conda", "credentials")]
    #[case::dot_segments("https://example.test/t/private-token/../../channel", "credentials")]
    #[case::encoded_dot_segments(
        "https://example.test/t/private-token/%2e%2e/%2e%2e/channel",
        "credentials"
    )]
    #[case::relative_token("//example.test/t/private-token/channel", "credentials")]
    #[case::relative_userinfo("//user:private-token@example.test/channel", "credentials")]
    fn artifact_urls_reject_credentials_without_echoing_them(
        #[case] value: &str,
        #[case] reason: &str,
    ) {
        let error = validate_artifact_url(value, "artifact URL").unwrap_err();

        assert!(error.to_string().contains(reason));
        assert!(!format!("{error:?}").contains("private-token"));
        assert!(!format!("{error:?}").contains(value));
    }

    #[rstest::rstest]
    #[case::https("https://example.test/channel/linux-64/pkg.conda")]
    #[case::http("http://example.test/channel/linux-64/pkg.conda")]
    #[case::file("file:///channel/linux-64/pkg.conda")]
    #[case::escaped_space("file:///channel%20name/linux-64/pkg.conda")]
    #[case::embedded_t("https://example.test/team/pkg.conda")]
    #[case::t_without_token("https://example.test/channel/t/")]
    #[case::encoded_percent("https://example.test/channel%2520name/pkg.conda")]
    #[case::channel_name("conda-forge")]
    #[case::relative_channel("//example.test/channel")]
    fn artifact_urls_accept_credential_free_paths(#[case] value: &str) {
        validate_artifact_url(value, "artifact URL").unwrap();
    }

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
    #[case(download_client)]
    #[case(runtime_update_client)]
    #[case(runtime_update_probe_client)]
    #[tokio::test]
    async fn auth_file_credentials_are_applied_to_downloads(
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
