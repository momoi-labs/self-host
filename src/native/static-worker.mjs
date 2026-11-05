// The wrapper Worker that turns workerd's bare DiskDirectory service into a
// static site: it fills in Content-Type and rewrites unknown paths to
// index.html when the site asks for single-page-application fallback.
//
// The workerd config binds the built assets directory as the "assets" service
// and passes NOT_FOUND_HANDLING as a text binding. The disk service maps a
// request path directly to a file under that directory and answers 404 for a
// missing one, so this Worker only has to proxy and decorate.

const MIME = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  json: "application/json; charset=utf-8",
  txt: "text/plain; charset=utf-8",
  csv: "text/csv; charset=utf-8",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  avif: "image/avif",
  ico: "image/x-icon",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  otf: "font/otf",
  eot: "application/vnd.ms-fontobject",
  wasm: "application/wasm",
  map: "application/json",
  webmanifest: "application/manifest+json",
  xml: "application/xml",
  pdf: "application/pdf",
  mp4: "video/mp4",
  webm: "video/webm",
  mp3: "audio/mpeg",
};

function contentType(name) {
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  return MIME[name.slice(dot + 1).toLowerCase()] || null;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    let pathname;
    try {
      pathname = decodeURIComponent(url.pathname);
    } catch {
      return new Response("Bad request", { status: 400 });
    }
    if (pathname === "" || pathname === "/") pathname = "/index.html";
    else if (pathname.endsWith("/")) pathname += "index.html";
    const upstream = new Request(new URL(pathname, "http://assets"), request);
    const response = await env.ASSETS.fetch(upstream);
    if (response.status !== 404) {
      const headers = new Headers(response.headers);
      const type = contentType(pathname);
      if (type && headers.get("content-type") === "application/octet-stream") {
        headers.set("content-type", type);
      }
      return new Response(response.body, { status: response.status, headers });
    }
    if (env.NOT_FOUND_HANDLING === "spa") {
      const index = await env.ASSETS.fetch(
        new Request(new URL("/index.html", "http://assets"), request),
      );
      if (index.status === 200) {
        const headers = new Headers(index.headers);
        headers.set("content-type", "text/html; charset=utf-8");
        headers.set("cache-control", "no-cache");
        return new Response(index.body, { status: 200, headers });
      }
    }
    return new Response("Not found", {
      status: 404,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  },
};
