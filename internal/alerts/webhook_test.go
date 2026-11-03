package alerts

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/otterscope/otterscope/internal/store"
)

// stubLookup returns a resolver that answers every host with ips. Tests use it
// so hostname validation is hermetic and so a name can be made to resolve to
// whatever the test needs.
func stubLookup(t *testing.T, ips ...string) lookupIPAddr {
	t.Helper()
	addrs := make([]net.IPAddr, 0, len(ips))
	for _, s := range ips {
		ip := net.ParseIP(s)
		if ip == nil {
			t.Fatalf("bad test IP %q", s)
		}
		addrs = append(addrs, net.IPAddr{IP: ip})
	}
	return func(context.Context, string) ([]net.IPAddr, error) { return addrs, nil }
}

// failLookup fails the test if validation resolves anything: a literal address
// must be judged without a lookup.
func failLookup(t *testing.T) lookupIPAddr {
	t.Helper()
	return func(_ context.Context, host string) ([]net.IPAddr, error) {
		t.Fatalf("unexpected DNS lookup for %q", host)
		return nil, nil
	}
}

// A rule created through the API must not be able to point the watcher back at
// the host it runs on, or at a cloud metadata service.
func TestValidateRejectsInternalWebhookAtCreation(t *testing.T) {
	base := store.Rule{
		Name: "n", Type: "error_rate", Threshold: 0.1, WindowSecs: 3600,
	}
	for _, url := range []string{
		"http://127.0.0.1:9000/hook",
		"http://169.254.169.254/latest/meta-data/iam/security-credentials/",
		"http://10.0.0.5/hook",
		"http://[fd00::1]/hook",
		"hooks.slack.com/services/T/B/x",
	} {
		r := base
		r.WebhookURL = url
		if err := Validate(r); err == nil {
			t.Errorf("Validate(%q) accepted an unsafe webhook", url)
		}
	}

	r := base
	r.WebhookURL = "https://93.184.216.34/hook"
	if err := Validate(r); err != nil {
		t.Errorf("Validate rejected a public webhook: %v", err)
	}
}

func TestValidateWebhookURLRejectsInternalAddresses(t *testing.T) {
	bad := []string{
		"http://127.0.0.1/hook",
		"http://127.1.2.3:8080/hook",
		"http://[::1]/hook",
		"http://[::ffff:127.0.0.1]/hook", // IPv4-mapped loopback
		"http://169.254.169.254/latest/meta-data/",
		"http://[fe80::1]/hook",
		"http://10.1.2.3/hook",
		"http://172.16.0.9/hook",
		"http://192.168.1.1/hook",
		"http://[fd00::1]/hook",
		"http://0.0.0.0:8080/hook",
		"http://100.64.0.1/hook",
	}
	for _, url := range bad {
		if err := validateWebhookURL(context.Background(), url, failLookup(t)); err == nil {
			t.Errorf("%q accepted, want refusal", url)
		}
	}
}

func TestValidateWebhookURLRequiresAbsoluteHTTPURL(t *testing.T) {
	bad := []string{
		"",
		"   ",
		"hooks.slack.com/services/T/B/x",
		"/hook",
		"//hooks.slack.com/x",
		"ftp://93.184.216.34/hook",
		"http:///hook",
		"https://user:pw@93.184.216.34/hook",
	}
	for _, url := range bad {
		if err := validateWebhookURL(context.Background(), url, failLookup(t)); err == nil {
			t.Errorf("%q accepted, want refusal", url)
		}
	}
}

func TestValidateWebhookURLAcceptsPublicEndpoints(t *testing.T) {
	cases := []struct {
		url string
		ips []string
	}{
		{"https://hooks.slack.com/services/T000/B000/xxxx", []string{"93.184.216.34"}},
		{"https://discord.com/api/webhooks/1/abc", []string{"2606:2800:220:1:248:1893:25c8:1946"}},
		{"https://93.184.216.34/hook?token=x", nil}, // literal: no lookup
	}
	for _, c := range cases {
		lookup := failLookup(t)
		if c.ips != nil {
			lookup = stubLookup(t, c.ips...)
		}
		if err := validateWebhookURL(context.Background(), c.url, lookup); err != nil {
			t.Errorf("%q rejected: %v", c.url, err)
		}
	}
}

// A name that resolves into internal space must be refused, and one that mixes
// public and internal answers must be refused too: that is the rebinding case.
func TestValidateWebhookURLRejectsInternalResolution(t *testing.T) {
	cases := [][]string{
		{"127.0.0.1"},
		{"169.254.169.254"},
		{"::1"},
		{"10.0.0.7", "93.184.216.34"},
	}
	for _, ips := range cases {
		if err := validateWebhookURL(context.Background(), "https://hook.example.com/x", stubLookup(t, ips...)); err == nil {
			t.Errorf("host resolving to %v accepted, want refusal", ips)
		}
	}
}

// A public endpoint that answers with a redirect into loopback must not be
// followed: Go rewrites a redirected POST to GET, so otherwise the watcher
// would happily read GET-only internal endpoints (the metadata service being
// the one that matters).
func TestWebhookClientDoesNotFollowRedirectIntoInternal(t *testing.T) {
	var hits int32
	internal := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		w.WriteHeader(http.StatusOK)
	}))
	defer internal.Close()

	redirector := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, internal.URL+"/latest/meta-data/iam/security-credentials/", http.StatusFound)
	}))
	defer redirector.Close()

	client := newWebhookClient()
	resp, err := client.Post(redirector.URL+"/hook", "application/json", strings.NewReader("{}"))
	if err == nil {
		resp.Body.Close()
		t.Fatal("client followed a redirect into loopback")
	}
	if !strings.Contains(err.Error(), "refusing redirect") {
		t.Fatalf("error = %v, want a refused redirect", err)
	}
	if got := atomic.LoadInt32(&hits); got != 0 {
		t.Fatalf("internal endpoint received %d requests, want 0", got)
	}
}

func TestWebhookRedirectsAreCapped(t *testing.T) {
	via := make([]*http.Request, maxWebhookRedirects+1)
	req := httptest.NewRequest(http.MethodPost, "https://93.184.216.34/hook", nil)
	err := checkWebhookRedirect(req, via)
	if err == nil {
		t.Fatal("redirect chain past the cap was accepted")
	}
	if !strings.Contains(err.Error(), "redirect chain") {
		t.Errorf("error = %v, want a redirect-chain error", err)
	}

	// One hop to a public address is still fine.
	fresh := httptest.NewRequest(http.MethodPost, "https://93.184.216.34/next", nil)
	if err := checkWebhookRedirect(fresh, via[:1]); err != nil {
		t.Errorf("public redirect refused: %v", err)
	}
}

func TestRedactWebhookURL(t *testing.T) {
	cases := []struct{ raw, want string }{
		{"https://hooks.slack.com/services/T000/B000/SUPERSECRET", "https://hooks.slack.com/***"},
		{"https://discord.com/api/webhooks/1/SUPERSECRET", "https://discord.com/***"},
		{"http://93.184.216.34:8443/hook?token=SUPERSECRET", "http://93.184.216.34:8443/***"},
		{"https://93.184.216.34", "https://93.184.216.34"},
		{"https://93.184.216.34/", "https://93.184.216.34"},
		{"", ""},
		{"not a url", "***"},
	}
	for _, c := range cases {
		got := RedactWebhookURL(c.raw)
		if got != c.want {
			t.Errorf("RedactWebhookURL(%q) = %q, want %q", c.raw, got, c.want)
		}
		if strings.Contains(got, "SUPERSECRET") {
			t.Errorf("RedactWebhookURL(%q) leaked the secret", c.raw)
		}
	}
}

// Guard against a future edit that makes redaction depend on parsing quirks:
// every output for a URL with a path must still name the host and mask the
// path.
func TestRedactWebhookURLKeepsHostMasksPath(t *testing.T) {
	got := RedactWebhookURL("https://hooks.example.com/a/b/c/d")
	if !strings.Contains(got, "hooks.example.com") || !strings.Contains(got, webhookPathMask) {
		t.Fatalf("redacted form %q lost the host or the mask", got)
	}
}
