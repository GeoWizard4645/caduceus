// Serves website/ at both caduceus.vivaanshahani.com and vivaanshahani.com/caduceus.
//
// Static assets are matched first by the runtime; this script only runs for
// requests that didn't match a file, which is exactly the /caduceus/* prefix
// and pretty paths like /features.

const PREFIX = "/caduceus";

function rewritePath(pathname) {
  if (pathname === "/features" || pathname === "/features/") {
    return "/features.html";
  }
  if (pathname === "/configure-ai" || pathname === "/configure-ai/") {
    return "/configure-ai.html";
  }
  if (pathname === "/remote" || pathname === "/remote/") {
    return "/remote.html";
  }
  return pathname;
}

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
      url.pathname = rewritePath(url.pathname.slice(PREFIX.length));
      return env.ASSETS.fetch(new Request(url, request));
    }

    const rewritten = rewritePath(url.pathname);
    if (rewritten !== url.pathname) {
      url.pathname = rewritten;
      return env.ASSETS.fetch(new Request(url, request));
    }

    return env.ASSETS.fetch(request);
  },
};
