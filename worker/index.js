// Serves website/ at both caduceus.vivaanshahani.com and vivaanshahani.com/caduceus.
//
// Static assets are matched first by the runtime; this script only runs for
// requests that didn't match a file, which is exactly the /caduceus/* prefix.

const PREFIX = "/caduceus";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // index.html references assets relatively (caduceus-mark.png), so the
    // trailing slash matters: at /caduceus the browser would resolve them
    // against the site root and 404.
    if (url.pathname === PREFIX) {
      return Response.redirect(`${url.origin}${PREFIX}/${url.search}`, 301);
    }

    if (url.pathname.startsWith(`${PREFIX}/`)) {
      url.pathname = url.pathname.slice(PREFIX.length);
      return env.ASSETS.fetch(new Request(url, request));
    }

    return env.ASSETS.fetch(request);
  },
};
