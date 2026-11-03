package server

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/otterscope/otterscope/internal/store"
)

// alertJSON builds a create-alert body in the shape the UI sends.
func alertJSON(t *testing.T, name, webhook string) string {
	t.Helper()
	b, err := json.Marshal(map[string]any{
		"project": "default", "name": name, "type": "error_rate",
		"threshold": 0.5, "windowSecs": 3600, "webhookUrl": webhook,
	})
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(b)
}

// Anyone who can create an alert can ask the watcher to POST somewhere within
// a minute, so an internal target has to be refused at creation rather than
// logged later (#128).
func TestCreateAlertRejectsUnsafeWebhook(t *testing.T) {
	srv, _ := testServer(t)
	for _, webhook := range []string{
		"http://127.0.0.1:9999/hook",
		"http://[::1]:9999/hook",
		"http://169.254.169.254/latest/meta-data/iam/security-credentials/",
		"http://10.0.0.5/hook",
		"http://[fd00::1]/hook",
		"hooks.slack.com/services/T000/B000/xxxx", // not absolute
	} {
		w := do(t, srv, http.MethodPost, "/api/alerts", alertJSON(t, "a", webhook))
		if w.Code != http.StatusBadRequest {
			t.Errorf("create with %q = %d, want 400 (%s)", webhook, w.Code, strings.TrimSpace(w.Body.String()))
		}
	}
}

func TestCreateAlertAcceptsPublicWebhook(t *testing.T) {
	srv, _ := testServer(t)
	// A public IP literal keeps the test off DNS; the https scheme is what the
	// check must not reject.
	w := do(t, srv, http.MethodPost, "/api/alerts", alertJSON(t, "ok", "https://93.184.216.34/hook"))
	if w.Code != http.StatusCreated {
		t.Fatalf("create = %d (%s)", w.Code, strings.TrimSpace(w.Body.String()))
	}
}

// The list endpoint is readable by any reader, and a Slack or Discord webhook
// path is the credential itself: the response may name the host, never the
// token.
func TestListAlertsRedactsWebhookSecret(t *testing.T) {
	const secret = "SUPERSECRETTOKEN"
	srv, st := testServer(t)
	if _, err := st.CreateAlert(t.Context(), store.Rule{
		Project: "default", Name: "leaky", Type: "error_rate", Threshold: 0.5,
		WindowSecs: 3600, Enabled: true,
		WebhookURL: "https://hooks.slack.com/services/T000/B000/" + secret,
	}); err != nil {
		t.Fatalf("CreateAlert: %v", err)
	}

	w := do(t, srv, http.MethodGet, "/api/alerts", "")
	if w.Code != http.StatusOK {
		t.Fatalf("list = %d (%s)", w.Code, strings.TrimSpace(w.Body.String()))
	}
	if strings.Contains(w.Body.String(), secret) {
		t.Fatalf("list leaked the webhook token: %s", w.Body.String())
	}
	var resp struct {
		Alerts []struct {
			WebhookURL string `json:"webhookUrl"`
		} `json:"alerts"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if len(resp.Alerts) != 1 {
		t.Fatalf("got %d alerts, want 1", len(resp.Alerts))
	}
	got := resp.Alerts[0].WebhookURL
	if got != "https://hooks.slack.com/***" {
		t.Errorf("webhookUrl = %q, want the host with a masked path", got)
	}
	if !strings.Contains(got, "hooks.slack.com") {
		t.Errorf("webhookUrl = %q, want the host kept so the destination is identifiable", got)
	}
}

// The create response is a read of the stored row too, so it must not echo the
// secret back either.
func TestCreateAlertResponseIsRedacted(t *testing.T) {
	const secret = "SUPERSECRETTOKEN"
	srv, _ := testServer(t)
	w := do(t, srv, http.MethodPost, "/api/alerts",
		alertJSON(t, "hook", "https://discord.com/api/webhooks/1/"+secret))
	if w.Code != http.StatusCreated {
		t.Fatalf("create = %d (%s)", w.Code, strings.TrimSpace(w.Body.String()))
	}
	if strings.Contains(w.Body.String(), secret) {
		t.Fatalf("create response leaked the webhook token: %s", strings.TrimSpace(w.Body.String()))
	}
}
