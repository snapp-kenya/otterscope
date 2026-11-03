package alerts

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// maxWebhookRedirects bounds how many redirects a webhook POST may follow.
// Every hop is re-validated, so this only exists to stop a redirect loop from
// holding a cycle open.
const maxWebhookRedirects = 3

// webhookLookupTimeout bounds the DNS lookup done while validating a URL, so a
// slow resolver cannot stall alert creation or a watcher cycle.
const webhookLookupTimeout = 5 * time.Second

// lookupIPAddr is *net.Resolver.LookupIPAddr narrowed to what validation needs.
// It is a parameter so tests can say what a name resolves to without touching
// the network, and so the production path and its test exercise the same code.
type lookupIPAddr func(ctx context.Context, host string) ([]net.IPAddr, error)

// ValidateWebhookURL reports whether raw is a webhook we are willing to POST
// to: an absolute http(s) URL whose host resolves only to public addresses.
//
// It runs both when an alert is created and before every POST. The second
// check is what stops rules that predate this guard — or that were written
// straight into the database — from turning the watcher into a request
// forwarder, and it re-resolves at use time so a name cannot be pointed at an
// internal address after the rule was accepted.
func ValidateWebhookURL(raw string) error {
	return validateWebhook(context.Background(), raw)
}

// validateWebhook is the production gate: it resolves with the system resolver.
func validateWebhook(ctx context.Context, raw string) error {
	return validateWebhookURL(ctx, raw, net.DefaultResolver.LookupIPAddr)
}

func validateWebhookURL(ctx context.Context, raw string, lookup lookupIPAddr) error {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return fmt.Errorf("webhook URL is empty")
	}
	u, err := url.Parse(raw)
	if err != nil {
		return fmt.Errorf("webhook URL is not a valid URL: %v", err)
	}
	// A relative or scheme-less value is not a mistake worth guessing at: the
	// client would resolve it against the server's own idea of a base.
	if !u.IsAbs() || u.Host == "" {
		return fmt.Errorf("webhook URL must be absolute, e.g. https://hooks.example.com/…")
	}
	switch strings.ToLower(u.Scheme) {
	case "http", "https":
	default:
		return fmt.Errorf("webhook URL must use http or https, not %q", u.Scheme)
	}
	if u.User != nil {
		return fmt.Errorf("webhook URL must not embed credentials")
	}
	host := u.Hostname()
	if host == "" {
		return fmt.Errorf("webhook URL has no host")
	}

	if ip := net.ParseIP(host); ip != nil {
		if err := checkWebhookAddr(ip); err != nil {
			return fmt.Errorf("webhook URL points at %v", err)
		}
		return nil
	}

	ctx, cancel := context.WithTimeout(ctx, webhookLookupTimeout)
	defer cancel()
	addrs, err := lookup(ctx, host)
	if err != nil {
		return fmt.Errorf("webhook URL host %q cannot be resolved: %v", host, err)
	}
	if len(addrs) == 0 {
		return fmt.Errorf("webhook URL host %q has no addresses", host)
	}
	// Refuse if any answer is internal. A name that resolves to both a public
	// and an internal address is the rebinding case; trusting the first answer
	// would only be as good as the resolver's ordering.
	for _, a := range addrs {
		if err := checkWebhookAddr(a.IP); err != nil {
			return fmt.Errorf("webhook URL host %q resolves to %v", host, err)
		}
	}
	return nil
}

// reservedWebhookNets are ranges that are not "private" to net.IP.IsPrivate
// but still must not be reachable from a webhook.
var reservedWebhookNets = mustCIDRs(
	"0.0.0.0/8",     // "this network": routes to localhost on Linux
	"100.64.0.0/10", // carrier-grade NAT; some clouds put metadata here
	"192.0.0.0/24",  // IETF protocol assignments
	"198.18.0.0/15", // benchmarking
	"240.0.0.0/4",   // reserved, includes broadcast
)

// checkWebhookAddr refuses addresses that are not publicly routable. The
// ranges are the ones that either loop back to the server or reach a cloud
// metadata service; 169.254.169.254 is covered by link-local.
func checkWebhookAddr(ip net.IP) error {
	if ip4 := ip.To4(); ip4 != nil {
		ip = ip4
	}
	switch {
	case ip.IsUnspecified():
		return fmt.Errorf("an unspecified address (%s)", ip)
	case ip.IsLoopback():
		return fmt.Errorf("a loopback address (%s)", ip)
	case ip.IsLinkLocalUnicast(), ip.IsLinkLocalMulticast():
		return fmt.Errorf("a link-local address (%s)", ip)
	case ip.IsPrivate(): // RFC 1918 and IPv6 unique-local (fc00::/7)
		return fmt.Errorf("a private address (%s)", ip)
	case ip.IsMulticast():
		return fmt.Errorf("a multicast address (%s)", ip)
	}
	for _, n := range reservedWebhookNets {
		if n.Contains(ip) {
			return fmt.Errorf("a reserved address (%s)", ip)
		}
	}
	return nil
}

func mustCIDRs(cidrs ...string) []*net.IPNet {
	out := make([]*net.IPNet, 0, len(cidrs))
	for _, c := range cidrs {
		_, n, err := net.ParseCIDR(c)
		if err != nil {
			panic("alerts: bad CIDR " + c)
		}
		out = append(out, n)
	}
	return out
}

// newWebhookClient is the HTTP client the watcher POSTs with. Each redirect is
// re-validated and the chain is capped, so a public endpoint cannot bounce the
// POST into loopback or link-local space (which would also let it reach
// GET-only internal endpoints, since Go rewrites a redirected POST to GET).
func newWebhookClient() *http.Client {
	return &http.Client{
		Timeout:       10 * time.Second,
		CheckRedirect: checkWebhookRedirect,
	}
}

func checkWebhookRedirect(req *http.Request, via []*http.Request) error {
	if len(via) > maxWebhookRedirects {
		return fmt.Errorf("webhook redirect chain longer than %d hops", maxWebhookRedirects)
	}
	if err := validateWebhook(req.Context(), req.URL.String()); err != nil {
		return fmt.Errorf("refusing redirect: %v", err)
	}
	return nil
}

// webhookPathMask stands in for the path and query of a stored webhook URL.
const webhookPathMask = "/***"

// RedactWebhookURL renders a stored webhook URL for a client: scheme, host and
// port, with the path and query masked. Slack, Discord and most other services
// put the token that authorises the POST in the path, so that part never leaves
// the server. A value that cannot be parsed is masked whole rather than echoed.
func RedactWebhookURL(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ""
	}
	u, err := url.Parse(raw)
	if err != nil || u.Scheme == "" || u.Host == "" {
		return "***"
	}
	out := u.Scheme + "://" + u.Host
	if u.Path != "" && u.Path != "/" {
		out += webhookPathMask
	}
	return out
}
