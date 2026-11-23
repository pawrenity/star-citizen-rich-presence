// Developer tool only: collect Star Citizen Wiki artwork choices for ships and contract types.
// Images stay hosted on media.starcitizen.tools (Discord can only show public URLs);
// only their URLs are bundled. Choice 1 is the default; users pick others in scrp.toml.
// Also writes assets/artwork-gallery.html for browsing the choices.
// node scripts/refresh-artwork.mjs   (or: bun scripts/refresh-artwork.mjs)
import { readFile, writeFile, rename } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join, dirname } from 'node:path';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const api = 'https://api.star-citizen.wiki/api';
const wiki = 'https://starcitizen.tools/api.php';
const headers = { 'User-Agent': 'star-citizen-rich-presence artwork refresh (developer tool)' };
const MAX_CHOICES = 16;

async function json(url) {
  // Be polite to the wiki: pace requests and honour rate limits.
  await new Promise(r => setTimeout(r, 250));
  for (let attempt = 1; ; attempt++) {
    const response = await fetch(url, { headers, signal: AbortSignal.timeout(30000) });
    if (response.ok) return response.json();
    if (attempt >= 6 || ![429, 500, 502, 503, 504].includes(response.status)) throw new Error(`${url}: HTTP ${response.status}`);
    const retry = Number(response.headers.get('retry-after'));
    await new Promise(r => setTimeout(r, (retry > 0 ? retry : 5 * attempt) * 1000));
  }
}
// Cache-busting query strings change on every re-upload; the path alone is stable.
const clean = url => url?.replace(/\?[^/]*$/, '');
const chunks = (list, size) => Array.from({ length: Math.ceil(list.length / size) }, (_, i) => list.slice(i * size, i * size + size));

/** Resolves titles through normalisation and redirects; returns Map(requested -> final title). */
function aliases(batch, query) {
  const alias = new Map(batch.map(t => [t, t]));
  for (const { from, to } of [...(query.normalized ?? []), ...(query.redirects ?? [])])
    for (const [title, current] of alias) if (current === from) alias.set(title, to);
  return alias;
}

/** Lead image of each page that exists. */
async function pageImages(titles) {
  const found = new Map();
  for (const batch of chunks(titles, 50)) {
    const { query } = await json(`${wiki}?action=query&format=json&prop=pageimages&piprop=thumbnail|name&pithumbsize=480&pilimit=max&redirects=1&titles=${encodeURIComponent(batch.join('|'))}`);
    const pages = new Map(Object.values(query.pages ?? {}).filter(p => p.thumbnail && !/\.svg$/i.test(clean(p.thumbnail.source))).map(p => [p.title, { url: clean(p.thumbnail.source), file: `File:${p.pageimage.replace(/_/g, ' ')}` }]));
    for (const [title, current] of aliases(batch, query)) if (pages.has(current)) found.set(title, { ...pages.get(current), page: current });
  }
  return found;
}

/** Files used on each (final) page title, in page order. */
async function pageFiles(titles) {
  const files = new Map(titles.map(t => [t, []]));
  for (const batch of chunks(titles, 50)) {
    let cont = {};
    do {
      const params = new URLSearchParams({ action: 'query', format: 'json', prop: 'images', imlimit: 'max', titles: batch.join('|'), ...cont });
      const response = await json(`${wiki}?${params}`);
      for (const page of Object.values(response.query.pages ?? {}))
        for (const image of page.images ?? []) files.get(page.title)?.push(image.title);
      cont = response.continue ?? null;
    } while (cont);
  }
  return files;
}

const SKIP = /logo|icon|lucide|badge|flag|signature|button|placeholder|\.svg$|\.gif$/i;
/** 480px raster thumbnails for files that look like artwork (wide enough, not UI glyphs). */
async function fileThumbs(files, { allowLogos = false } = {}) {
  const thumbs = new Map();
  const wanted = [...new Set(files)].filter(f => allowLogos ? !/\.svg$|\.gif$/i.test(f) : !SKIP.test(f));
  for (const batch of chunks(wanted, 50)) {
    const params = new URLSearchParams({ action: 'query', format: 'json', prop: 'imageinfo', iiprop: 'url|size|mime', iiurlwidth: '480', titles: batch.join('|') });
    const { query } = await json(`${wiki}?${params}`);
    const final = aliases(batch, query);
    const info = new Map(Object.values(query.pages ?? {}).map(p => [p.title, p.imageinfo?.[0]]));
    for (const [file, title] of final) {
      const i = info.get(title);
      if (!i || !/^image\/(jpeg|png|webp)$/.test(i.mime)) continue;
      if (!allowLogos && (i.width < 400 || i.height < 220)) continue;
      const url = clean(i.thumburl ?? i.url);
      // Discord rejects image URLs over 256 bytes; skip full-size originals over 1 MB.
      if (url.length > 256 || (!i.thumburl?.includes("/thumb/") && i.size > 1_000_000)) continue;
      thumbs.set(file, url);
    }
  }
  return thumbs;
}

// Wiki page titles (without manufacturer) keyed by class name, from the pinned game version.
const source = JSON.parse(await readFile(join(root, 'data/content-source.json'), 'utf8'));
const titleByClass = new Map();
for (let page = 1, last = 1; page <= last; page++) {
  const response = await json(`${api}/vehicles?version=${encodeURIComponent(source.game_version)}&page%5Bsize%5D=200&page%5Bnumber%5D=${page}`);
  last = response.meta.last_page;
  for (const row of response.data) if (row.class_name && row.name) titleByClass.set(row.class_name, row.name);
}

// Ships the pinned catalogue does not know yet; unknown classes display as "MANUFACTURER Name".
const EXTRA_SHIPS = { 'RSI Galaxy': 'Galaxy' };

const vehicles = JSON.parse(await readFile(join(root, 'data/vehicles.json'), 'utf8'));
// Manufacturer words lead many display names ("Aegis", "MISC"); hull names lead few.
const firstWords = new Map();
for (const name of new Set(vehicles.map(v => v.name))) {
  const word = name.split(' ')[0];
  firstWords.set(word, (firstWords.get(word) ?? 0) + 1);
}
const manufacturers = new Set([...firstWords].filter(([, n]) => n >= 5).map(([w]) => w));
const wanted = new Map(); // display name -> candidate wiki titles, most specific first
for (const { name, class_name } of vehicles) {
  // Wiki pages omit the manufacturer; a few API rows still include it.
  let title = titleByClass.get(class_name) ?? name;
  const [first, ...rest] = title.split(' ');
  if (manufacturers.has(first) && rest.length) title = rest.join(' ');
  if (!title || wanted.has(name)) continue;
  const words = title.split(' ');
  // Special editions and paints fall back to their base hull's page.
  wanted.set(name, words.map((_, i) => words.slice(0, words.length - i).join(' ')));
}
for (const [name, title] of Object.entries(EXTRA_SHIPS)) wanted.set(name, [title]);

const leads = await pageImages([...new Set([...wanted.values()].flat())]);
const shipPage = new Map();
for (const [name, candidates] of wanted) {
  const hit = candidates.find(title => leads.has(title));
  if (hit) shipPage.set(name, leads.get(hit));
}
const shipFiles = await pageFiles([...new Set([...shipPage.values()].map(l => l.page))]);
const shipThumbs = await fileThumbs([...shipFiles.values()].flat());

function choices(lead, files, thumbs) {
  const list = lead ? [lead] : [];
  for (const file of files) {
    const url = thumbs.get(file);
    if (url && !list.includes(url)) list.push(url);
    if (list.length >= MAX_CHOICES) break;
  }
  return list;
}
const shipArt = {};
for (const name of [...shipPage.keys()].sort((a, b) => a.localeCompare(b))) {
  const lead = shipPage.get(name);
  shipArt[name] = choices(lead.url, shipFiles.get(lead.page) ?? [], shipThumbs);
}

// Contract categories: an in-game reputation logo (where the wiki has one) first,
// then artwork from related gameplay pages. Keys match src/artwork.rs.
const missionSources = {
  bounty: { logo: 'BHG logo 3.13.png', pages: ['Bounty Hunters Guild', 'Bounty hunting'] },
  mercenary: { logo: 'Mercenary Guild logo RepUI.png', pages: ['Mercenary Guild', 'Mercenary'] },
  hauling: { logo: 'Interstellar Transport Guild logo RepUI.png', pages: ['Interstellar Transport Guild', 'Hauling'] },
  delivery: { logo: 'Covalexshipping logo.png', pages: ['Covalex Shipping'] },
  mining: { logo: 'Shubin logo circle.png', pages: ['Shubin Interstellar', 'Mining'] },
  salvage: { pages: ['Salvage'] },
  rescue: { pages: ['Apollo Medivac', 'Medical'] },
  investigation: { pages: ['Recovery Contract'] },
  racing: { pages: ['Racing'] },
  xenothreat: { pages: ['XenoThreat'] },
  wikelo: { pages: ['Wikelo'] },
  arena: { pages: ['Arena Commander'] },
};
const logos = await fileThumbs(Object.values(missionSources).filter(s => s.logo).map(s => `File:${s.logo}`), { allowLogos: true });
const missionLeads = await pageImages([...new Set(Object.values(missionSources).flatMap(s => s.pages))]);
const missionFiles = await pageFiles([...new Set([...missionLeads.values()].map(l => l.page))]);
const missionThumbs = await fileThumbs([...missionFiles.values()].flat());
const missionArt = {};
for (const [category, s] of Object.entries(missionSources)) {
  const lead = s.logo ? logos.get(`File:${s.logo}`) : missionLeads.get(s.pages[0])?.url;
  if (!lead) throw new Error(`No raster wiki image for mission category ${category}`);
  const files = s.pages.flatMap(p => missionFiles.get(missionLeads.get(p)?.page) ?? []);
  const leadsOfPages = s.pages.map(p => missionLeads.get(p)?.url).filter(Boolean);
  const list = choices(lead, [], new Map());
  for (const url of leadsOfPages) if (!list.includes(url)) list.push(url);
  missionArt[category] = [...list, ...choices(null, files, missionThumbs).filter(u => !list.includes(u))].slice(0, MAX_CHOICES);
}

async function save(path, text) {
  await writeFile(`${path}.tmp`, text);
  await rename(`${path}.tmp`, path);
}
await save(join(root, 'data/ship-artwork.json'), `${JSON.stringify(shipArt, null, 1)}\n`);
await save(join(root, 'data/mission-artwork.json'), `${JSON.stringify(missionArt, null, 1)}\n`);

const template = await readFile(join(root, 'scripts/artwork-gallery.template.html'), 'utf8');
const data = JSON.stringify({ ships: shipArt, missions: missionArt, game_version: source.game_version }).replace(/</g, '\\u003c');
await save(join(root, 'assets/artwork-gallery.html'), template.replace('/*ARTWORK_DATA*/null', data));

const names = new Set([...vehicles.map(v => v.name), ...Object.keys(EXTRA_SHIPS)]);
const total = Object.values(shipArt).reduce((n, l) => n + l.length, 0);
console.log(`ships with artwork: ${Object.keys(shipArt).length}/${names.size} (${total} choices)`);
console.log(`missing: ${[...names].filter(n => !shipArt[n]).join(', ')}`);
console.log(`mission categories: ${Object.entries(missionArt).map(([k, v]) => `${k}=${v.length}`).join(' ')}`);
