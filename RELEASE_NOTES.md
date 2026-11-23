## 3.1.4: reliable chunked image uploads with automatic fallback

- **Chunked WinHTTP uploads:** switched image uploads from single-shot `WinHttpSendRequest` to 64 KB chunked `WinHttpWriteData` streaming, fixing empty/dropped responses on larger images.
- **Automatic retry & fallback host:** retries `catbox.moe` once on transient empty responses and automatically falls back to `pomf2.lain.la` so custom image uploads stay reliable.

Download the installer or the Windows ZIP under **Assets**, or click **Update now** inside the app.
