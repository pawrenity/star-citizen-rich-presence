## 3.0.0: ship art, mission icons, parties, and your own images

- **Ship artwork:** while you're aboard, the big image is your ship's Star Citizen Wiki artwork (282 ships, including the Galaxy). The location image moves to the small badge.
- **Mission icons:** active contracts get a badge for their type: bounty, mercenary, hauling, delivery, mining, salvage, rescue, investigation, racing, XenoThreat, Wikelo, or Arena Commander.
- **Party status:** the card shows "In a party" while you're in one. Turn it off with `show_party = false`.
- **Choose your images:** open `artwork-gallery.html`, click any wiki picture, and paste the copied line into `scrp.toml` under `[ship_images]` or `[mission_images]`. You can also use your own `https://` image or a Discord asset key.
- **Activity type:** set `activity_type` to `playing`, `listening`, `watching`, or `competing`. `status_display` chooses what the member list shows after it. See the README to show your own game name, such as "Listening to Scam Citizen".
- **Fix:** contract titles no longer show raw in-game markup like `<EM4>[BP]</EM4>`.
- Rewritten README and small performance fixes.

Download the Windows ZIP under **Assets**, extract it, and launch the executable. Quit the previous companion before upgrading. Desktop preferences carry over.
