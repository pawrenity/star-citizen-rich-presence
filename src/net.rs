//! Minimal HTTPS GET and POST over WinHTTP (no extra dependencies).
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

/// One request with its parents. Fields drop in declaration order, so the request closes
/// before its connection, and the connection before its session.
struct Exchange {
    request: Handle,
    _connection: Handle,
    _session: Handle,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn last_error(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} failed: {}", std::io::Error::last_os_error())
}

/// Opens session, connection, and request for `method` on an HTTPS `url`. Timeouts are in
/// milliseconds: resolve, connect, send, receive.
fn open(method: &str, url: &str, timeouts: (i32, i32, i32, i32)) -> Result<Exchange> {
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
        WinHttpSetTimeouts(session.0, timeouts.0, timeouts.1, timeouts.2, timeouts.3);
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
            wide(method).as_ptr(),
            wide(&path).as_ptr(),
            null(),
            null(),
            accept.as_ptr(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("Opening the request"));
        }
        Ok(Exchange {
            request,
            _connection: connection,
            _session: session,
        })
    }
}

/// Checks for HTTP 200 and reads the response body, failing if it exceeds `limit` bytes.
/// `party` names the server in error messages.
fn read_body(request: &Handle, limit: usize, party: &str) -> Result<Vec<u8>> {
    let request = request.0;
    unsafe {
        let mut status = 0u32;
        let mut size = 4u32;
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        );
        if status != 200 {
            bail!("{party} answered HTTP {status}");
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request, &mut available) == 0 {
                return Err(last_error("Reading response"));
            }
            if available == 0 {
                break;
            }
            let start = body.len();
            ensure!(
                start + available as usize <= limit,
                "{party} response is unexpectedly large"
            );
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(
                request,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            ) == 0
            {
                return Err(last_error("Reading response"));
            }
            body.truncate(start + read as usize);
        }
        Ok(body)
    }
}

/// HTTPS GET with redirects (GitHub downloads redirect to its CDN), up to `limit` bytes.
pub fn get(url: &str, limit: usize) -> Result<Vec<u8>> {
    fetch(url, limit).with_context(|| format!("Could not download {url}"))
}

fn fetch(url: &str, limit: usize) -> Result<Vec<u8>> {
    let exchange = open("GET", url, (10_000, 10_000, 15_000, 30_000))?;
    let headers = wide("Accept: application/vnd.github+json\r\n");
    unsafe {
        if WinHttpSendRequest(
            exchange.request.0,
            headers.as_ptr(),
            u32::MAX,
            null(),
            0,
            0,
            0,
        ) == 0
            || WinHttpReceiveResponse(exchange.request.0, null_mut()) == 0
        {
            return Err(last_error("Downloading"));
        }
    }
    read_body(&exchange.request, limit, "Server")
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
    upload(url, boundary, &payload, limit).with_context(|| format!("Could not upload to {url}"))
}

fn upload(url: &str, boundary: &str, payload: &[u8], limit: usize) -> Result<Vec<u8>> {
    let total_len = u32::try_from(payload.len()).context("Upload is too large")?;
    let exchange = open("POST", url, (10_000, 10_000, 30_000, 60_000))?;
    let headers = wide(&format!(
        "Content-Type: multipart/form-data; boundary={boundary}\r\n"
    ));
    unsafe {
        let request = exchange.request.0;
        if WinHttpSendRequest(request, headers.as_ptr(), u32::MAX, null(), 0, total_len, 0) == 0 {
            return Err(last_error("Sending upload headers"));
        }
        let mut written_total = 0usize;
        while written_total < payload.len() {
            let chunk = &payload[written_total..(written_total + 65_536).min(payload.len())];
            let mut written = 0u32;
            if WinHttpWriteData(
                request,
                chunk.as_ptr().cast(),
                chunk.len() as u32,
                &mut written,
            ) == 0
            {
                return Err(last_error("Uploading"));
            }
            ensure!(written > 0, "Upload stalled");
            written_total += written as usize;
        }
        if WinHttpReceiveResponse(request, null_mut()) == 0 {
            return Err(last_error("Waiting for upload response"));
        }
    }
    read_body(&exchange.request, limit, "Upload host")
}

fn validate_upload_url(url: &str) -> Result<String> {
    let url = url.trim().to_string();
    ensure!(
        url.starts_with("https://") && url.len() <= 256 && !url.contains(char::is_whitespace),
        "Upload host returned unexpected response: {}",
        if url.is_empty() { "(empty)" } else { &url }
    );
    Ok(url)
}

fn upload_catbox(png_bytes: &[u8]) -> Result<String> {
    let response = post_multipart(
        "https://catbox.moe/user/api.php",
        &[("reqtype", "fileupload")],
        "fileToUpload",
        "image.png",
        "image/png",
        png_bytes,
        4096,
    )?;
    let text = String::from_utf8(response).context("Upload host returned non-UTF-8 response")?;
    validate_upload_url(&text)
}

fn upload_pomf(png_bytes: &[u8]) -> Result<String> {
    let response = post_multipart(
        "https://pomf2.lain.la/upload.php",
        &[],
        "files[]",
        "image.png",
        "image/png",
        png_bytes,
        16_384,
    )?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&response).context("Fallback upload host returned invalid JSON")?;
    let url = parsed["files"][0]["url"]
        .as_str()
        .context("Fallback upload host did not return a file URL")?;
    validate_upload_url(url)
}

/// Uploads a PNG image to catbox.moe (retrying once on transient errors, then falling back to
/// pomf2.lain.la) and returns its direct HTTPS URL.
pub fn upload_image(png_bytes: &[u8]) -> Result<String> {
    if let Ok(url) = upload_catbox(png_bytes).or_else(|_| upload_catbox(png_bytes)) {
        return Ok(url);
    }
    upload_pomf(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn live_catbox_upload() {
        let mut img = image::RgbaImage::new(512, 512);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([
                ((x * 37) ^ (y * 17)) as u8,
                (x + y * 53) as u8,
                (x * 13) as u8,
                255,
            ]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let bytes = out.into_inner();
        println!("uploading {} bytes", bytes.len());
        let catbox = upload_catbox(&bytes).unwrap();
        println!("catbox: {catbox}");
        assert!(
            catbox.starts_with("https://files.catbox.moe/"),
            "got {catbox}"
        );
        let pomf = upload_pomf(&bytes).unwrap();
        println!("pomf: {pomf}");
        assert!(pomf.starts_with("https://pomf2.lain.la/"), "got {pomf}");
    }
}
