// Releases download worker: serves release binaries from the `syscity-releases`
// R2 bucket under https://syscity.net/releases/<file>.
//
// Bucket layout is flat — the Release workflow (`release.yml`) syncs artifacts
// to the bucket root, and this worker maps /releases/<file> to object <file>.
// Filenames are stable across tags ("latest" semantics), so the cache TTL is
// short: a new release must become downloadable within minutes.

const PREFIX = "/releases/";
const CACHE_CONTROL = "public, max-age=300";

function downloadHeaders(object, key) {
  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("etag", object.httpEtag);
  headers.set("Cache-Control", CACHE_CONTROL);
  headers.set("Accept-Ranges", "bytes");
  headers.set("Content-Disposition", `attachment; filename="${key}"`);
  return headers;
}

export default {
  async fetch(request, env) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method Not Allowed", { status: 405 });
    }
    const url = new URL(request.url);
    if (!url.pathname.startsWith(PREFIX)) {
      return new Response("Not Found", { status: 404 });
    }
    const key = url.pathname.slice(PREFIX.length);
    // Flat bucket: reject empty keys, subpaths, and traversal attempts.
    if (!key || key.includes("/") || key.includes("..")) {
      return new Response("Not Found", { status: 404 });
    }

    if (request.method === "HEAD") {
      const head = await env.RELEASES.head(key);
      if (!head) {
        return new Response("Not Found", { status: 404 });
      }
      const headers = downloadHeaders(head, key);
      headers.set("Content-Length", head.size);
      return new Response(null, { headers });
    }

    // Pass the request headers so R2 honors Range (resumable downloads).
    // Only emit a 206 when the client actually asked for a range; R2 populates
    // `object.range` with the full span even for whole-object reads when given
    // a Headers object without a Range header.
    const requestedRange = request.headers.has("range");
    const object = await env.RELEASES.get(key, { range: request.headers });
    if (!object) {
      return new Response("Not Found", { status: 404 });
    }

    const headers = downloadHeaders(object, key);
    let status = 200;
    if (requestedRange && object.range && "offset" in object.range) {
      const { offset, length } = object.range;
      headers.set("Content-Range", `bytes ${offset}-${offset + length - 1}/${object.size}`);
      status = 206;
    }
    return new Response(object.body, { headers, status });
  },
};
