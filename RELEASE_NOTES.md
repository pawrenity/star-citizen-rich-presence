## 3.1.4: custom game name field and reliable chunked image uploads

- **Custom game name:** added a **Game name** field on the **Activity** page (and `custom_name` in `scrp.toml`) so you can override the displayed game title (e.g. *Scam Citizen*) — updates the live preview and sends `name` in the Discord RPC payload.
- **Icon-cropped image previews:** picture tiles on the **Images** page are now 1:1 square icon crops (27 per page, circular mask for contract badges) matching how Discord displays them, without the dotted focus box.
- **Periodic update checks & Windows notifications:** checks for updates every 30 minutes while running and pops a native Windows tray notification once per new release (clicking it opens the **Updates** tab).
- **Chunked WinHTTP uploads:** switched image uploads from single-shot `WinHttpSendRequest` to 64 KB chunked `WinHttpWriteData` streaming, fixing empty/dropped responses on larger images.
- **Automatic retry & fallback host:** retries `catbox.moe` once on transient empty responses and automatically falls back to `pomf2.lain.la` so custom image uploads stay reliable.

Download the installer or the Windows ZIP under **Assets**, or click **Update now** inside the app.
