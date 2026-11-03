package server

import (
	"mime"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
)

// allowedOriginsEnv lists extra origins a reverse-proxied deployment is served
// from, comma separated: "https://otterscope.example.com,https://obs.corp:8443".
// Loopback needs no entry; anything else does, because the API only answers a
// browser that proves it is already on this site (see guard).
const allowedOriginsEnv = "OTTERSCOPE_ALLOWED_ORIGINS"

// contentSecurityPolicy is strict on purpose: the built UI loads its JS and CSS
// as separate files under /assets, so 'self' is all it needs. Framing, <base>
// hijacking and plugin embedding are refused outright.
const contentSecurityPolicy = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'"

// securityHeaders stamps the headers every UI and JSON response carries. It
// runs before the handler so a handler can still override a value, and it does
// not depend on the request: a 404 and a run payload get the same policy.
func securityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("Content-Security-Policy", contentSecurityPolicy)
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("Referrer-Policy", "no-referrer")
		next.ServeHTTP(w, r)
	})
}

// guard is the browser-facing gate in front of /api and /mcp: the loopback
// Origin rule the MCP handler already applies, widened by a Host allow-list and
// OTTERSCOPE_ALLOWED_ORIGINS, plus JSON-only bodies for writes.
//
// Both checks are needed and they cover different attacks:
//
//   - Origin stops a page on another site from riding the user's network
//     position to this API (a form POST or a fetch with a foreign Origin).
//   - Host stops DNS rebinding, where the attacker's own page is the origin
//     *and* the host: the browser then omits Origin on a same-origin GET, so
//     only the Host allow-list still distinguishes evil.example from a name
//     this deployment actually answers to.
//
// Non-browser clients (curl, the SDKs) send no Origin, so only the Host rule
// applies to them — loopback, a LAN IP, or a host named in
// OTTERSCOPE_ALLOWED_ORIGINS. A client that can set Host can set anything, so
// there is nothing left to protect there; the rule exists for browsers.
func (s *Server) guard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !guardedPath(r.URL.Path) {
			next.ServeHTTP(w, r)
			return
		}
		if !s.hostAllowed(r.Host) {
			writeJSON(w, http.StatusForbidden, map[string]string{"error": "host not allowed"})
			return
		}
		if origin := r.Header.Get("Origin"); origin != "" && !s.originAllowed(origin, r.Host) {
			writeJSON(w, http.StatusForbidden, map[string]string{"error": "origin not allowed"})
			return
		}
		// A body-carrying write must say it is JSON, so a cross-site form post
		// (the one request shape a browser sends without a preflight) can never
		// be parsed as a command. Bodiless writes — DELETE, the share and
		// evaluate POSTs — carry no content type at all; the Origin and Host
		// checks above are what cover those.
		if apiWrite(r) && r.ContentLength != 0 && !jsonMediaType(r.Header.Get("Content-Type")) {
			writeJSON(w, http.StatusUnsupportedMediaType, map[string]string{"error": "writes must be application/json"})
			return
		}
		next.ServeHTTP(w, r)
	})
}

// guardedPath reports whether p is on the surface the guard owns. /metrics is
// not: it is handled by authWrap like any other token-protected read.
func guardedPath(p string) bool {
	return p == "/api" || strings.HasPrefix(p, "/api/") || p == "/mcp"
}

// apiWrite reports whether r mutates API state. MCP has its own protocol
// layer that rejects a non-JSON body, so the content-type rule is the API's.
func apiWrite(r *http.Request) bool {
	switch r.Method {
	case http.MethodPost, http.MethodPut, http.MethodPatch, http.MethodDelete:
		return strings.HasPrefix(r.URL.Path, "/api/")
	}
	return false
}

// jsonMediaType reports whether a Content-Type header is application/json,
// tolerating parameters ("; charset=utf-8").
func jsonMediaType(ct string) bool {
	if ct == "" {
		return false
	}
	mt, _, err := mime.ParseMediaType(ct)
	return err == nil && mt == "application/json"
}

// originAllowed accepts loopback origins, origins configured for this
// deployment, and a same-origin request to an IP-literal host (a LAN deployment
// served from its own address: there is no name there to rebind). The scheme
// must be http(s): "null" (sandboxed iframe, file://) and
// "chrome-extension://..." are refused.
func (s *Server) originAllowed(origin, requestHost string) bool {
	u, err := url.Parse(origin)
	if err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") {
		return false
	}
	if isLoopbackHost(u.Hostname()) {
		return true
	}
	if _, ok := s.allowedOrigins[originKey(u)]; ok {
		return true
	}
	// Same page, same address: the UI on http://192.168.1.10:8317 calling its
	// own API. An attacker's page is a different origin, so this does not
	// weaken the check — and a host reached by IP cannot be DNS-rebound.
	host := hostname(requestHost)
	if net.ParseIP(host) != nil && strings.EqualFold(hostname(u.Host), host) {
		return true
	}
	return false
}

// hostAllowed accepts a Host header naming loopback, an IP literal, or a
// configured origin's host. The port is ignored: a proxy may rewrite it, and
// the name is what rebinding attacks take over. A DNS name that was not
// configured is refused, because that is exactly what an attacker rebinds.
func (s *Server) hostAllowed(hostport string) bool {
	host := hostname(hostport)
	if host == "" {
		return false
	}
	if isLoopbackHost(host) || net.ParseIP(host) != nil {
		return true
	}
	_, ok := s.allowedHosts[host]
	return ok
}

// parseAllowedOrigins reads OTTERSCOPE_ALLOWED_ORIGINS into the origin and host
// sets the guard consults. Unparseable entries are skipped rather than admitted.
func parseAllowedOrigins(raw string) (origins, hosts map[string]struct{}) {
	origins, hosts = map[string]struct{}{}, map[string]struct{}{}
	for _, entry := range strings.Split(raw, ",") {
		entry = strings.TrimSpace(entry)
		if entry == "" {
			continue
		}
		u, err := url.Parse(entry)
		if err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") {
			continue
		}
		origins[originKey(u)] = struct{}{}
		hosts[strings.ToLower(u.Hostname())] = struct{}{}
	}
	return origins, hosts
}

// originKey normalizes an origin for comparison: lower case, default port
// dropped, no trailing slash or path.
func originKey(u *url.URL) string {
	scheme := strings.ToLower(u.Scheme)
	host := strings.ToLower(u.Hostname())
	port := u.Port()
	if (scheme == "http" && port == "80") || (scheme == "https" && port == "443") {
		port = ""
	}
	if port != "" {
		host = net.JoinHostPort(host, port)
	}
	return scheme + "://" + host
}

// hostname strips a port and any IPv6 brackets from a Host header value.
func hostname(hostport string) string {
	h := strings.ToLower(strings.TrimSpace(hostport))
	if host, _, err := net.SplitHostPort(h); err == nil {
		h = host
	}
	h = strings.TrimPrefix(h, "[")
	h = strings.TrimSuffix(h, "]")
	return strings.TrimSuffix(h, ".")
}

// isLoopbackHost reports whether host names this machine.
func isLoopbackHost(host string) bool {
	h := strings.Trim(strings.ToLower(host), "[]")
	if h == "localhost" || h == "::1" {
		return true
	}
	if ip := net.ParseIP(h); ip != nil {
		return ip.IsLoopback()
	}
	return false
}

// envAllowedOrigins is the startup snapshot of OTTERSCOPE_ALLOWED_ORIGINS. It
// is read once in New, so a long-running server never changes its allow-list
// under a request.
func envAllowedOrigins() (origins, hosts map[string]struct{}) {
	return parseAllowedOrigins(os.Getenv(allowedOriginsEnv))
}
