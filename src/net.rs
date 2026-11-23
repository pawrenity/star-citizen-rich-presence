//! Minimal HTTPS GET over WinHTTP (no extra dependencies).
#![cfg(windows)]
use anyhow::{Context, Result, bail, ensure};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Networking::WinHttp::*;

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn last_error(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} failed: {}", std::io::Error::last_os_error())
}

/// HTTPS GET with redirects (GitHub downloads redirect to its CDN), up to `limit` bytes.
pub fn get(url: &str, limit: usize) -> Result<Vec<u8>> {
    let rest = url
        .strip_prefix("https://")
        .context("Only HTTPS is allowed")?;
    let (host, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    unsafe {
        let agent = wide(concat!(
            "star-citizen-rich-presence/",
            env!("CARGO_PKG_VERSION")
        ));
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let connection = Handle(WinHttpConnect(
            session.0,
            wide(host).as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(last_error("Connecting"));
        }
        let any_type = wide("*/*");
        let accept = [any_type.as_ptr(), null()];
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide(&path).as_ptr(),
            null(),
            null(),
            accept.as_ptr(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("Opening the request"));
        }
        let headers = wide("Accept: application/vnd.github+json\r\n");
        if WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(last_error("Downloading"));
        }
        let mut status = 0u32;
        let mut size = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        );
        if status != 200 {
            bail!("Server answered HTTP {status}");
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err(last_error("Reading"));
            }
            if available == 0 {
                break;
            }
            let start = body.len();
            ensure!(
                start + available as usize <= limit,
                "Download is unexpectedly large"
            );
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            ) == 0
            {
                return Err(last_error("Reading"));
            }
            body.truncate(start + read as usize);
        }
        Ok::<_, anyhow::Error>(body)
    }
    .with_context(|| format!("Could not download {url}"))
}

/// HTTPS multipart/form-data POST over WinHTTP, returning up to `limit` response bytes.
pub fn post_multipart(
    url: &str,
    fields: &[(&str, &str)],
    file_field: &str,
    filename: &str,
    content_type: &str,
    data: &[u8],
    limit: usize,
) -> Result<Vec<u8>> {
    let rest = url
        .strip_prefix("https://")
        .context("Only HTTPS is allowed")?;
    let (host, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let boundary = "----SCRPFormBoundary7MA4YWxkTrZu0gW";
    let mut payload = Vec::with_capacity(data.len() + 512);
    for (name, value) in fields {
        payload.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    payload.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    payload.extend_from_slice(data);
    payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let total_len = u32::try_from(payload.len()).context("Upload is too large")?;

    unsafe {
        let agent = wide(concat!(
            "star-citizen-rich-presence/",
            env!("CARGO_PKG_VERSION")
        ));
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 60_000);
        let connection = Handle(WinHttpConnect(
            session.0,
            wide(host).as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(last_error("Connecting"));
        }
        let any_type = wide("*/*");
        let accept = [any_type.as_ptr(), null()];
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            wide("POST").as_ptr(),
            wide(&path).as_ptr(),
            null(),
            null(),
            accept.as_ptr(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("Opening the request"));
        }
        let headers = wide(&format!(
            "Content-Type: multipart/form-data; boundary={boundary}\r\n"
        ));
        if WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            u32::MAX,
            payload.as_ptr().cast(),
            total_len,
            total_len,
            0,
        ) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(last_error("Uploading"));
        }
        let mut status = 0u32;
        let mut size = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        );
        if status != 200 {
            bail!("Upload host answered HTTP {status}");
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err(last_error("Reading response"));
            }
            if available == 0 {
                break;
            }
            let start = body.len();
            ensure!(
                start + available as usize <= limit,
                "Upload response is unexpectedly large"
            );
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            ) == 0
            {
                return Err(last_error("Reading response"));
            }
            body.truncate(start + read as usize);
        }
        Ok::<_, anyhow::Error>(body)
    }
    .with_context(|| format!("Could not upload to {url}"))
}

/// Uploads a PNG image to catbox.moe and returns its direct HTTPS URL.
pub fn upload_image(png_bytes: &[u8]) -> Result<String> {
    let response = post_multipart(
        "https://catbox.moe/user/api.php",
        &[("reqtype", "fileupload")],
        "fileToUpload",
        "image.png",
        "image/png",
        png_bytes,
        4096,
    )?;
    let url = String::from_utf8(response)
        .context("Upload host returned non-UTF-8 response")?
        .trim()
        .to_string();
    ensure!(
        url.starts_with("https://") && url.len() <= 256 && !url.contains(char::is_whitespace),
        "Upload host returned unexpected response: {url}"
    );
    Ok(url)
}
