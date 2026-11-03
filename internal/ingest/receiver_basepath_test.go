package ingest

import (
	"bytes"
	"net/http"
	"net/http/httptest"
	"testing"

	"go.opentelemetry.io/collector/pdata/ptrace/ptraceotlp"
)

// postTo is post() with an explicit path. Under a base path the receiver's
// route moves, and which paths it answers is precisely what these tests are
// about — post() is hard-wired to the root.
func postTo(t *testing.T, h http.Handler, path, contentType string, body []byte) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodPost, path, bytes.NewReader(body))
	req.Header.Set("Content-Type", contentType)
	w := httptest.NewRecorder()
	h.ServeHTTP(w, req)
	return w
}

// TestOTLPReceiverUnderBasePath covers the deployment shape the prefix is for:
// OTEL_EXPORTER_OTLP_ENDPOINT=https://host/otterscope, and the exporter appends
// /v1/traces to it. The batch must be stored exactly as at the root, the root
// path must not be this app, and a bad body under the prefix must reach the
// decoder (routing matched) rather than 404.
func TestOTLPReceiverUnderBasePath(t *testing.T) {
	base := "/otterscope"
	sink := &countSink{}
	h := NewHandlerAt(base, sink, openResolver, 0, 0)

	w := postTo(t, h, base+"/v1/traces", ctJSON, fixture(t, "pydantic_ai_chat.json"))
	if w.Code != http.StatusOK {
		t.Fatalf("POST %s/v1/traces = %d, body %s", base, w.Code, w.Body.String())
	}
	if ct := w.Header().Get("Content-Type"); ct != ctJSON {
		t.Fatalf("response content type = %q, want %q", ct, ctJSON)
	}
	if got := sink.spans.Load(); got != fixtureSpanCount {
		t.Fatalf("sink got %d spans, want %d", got, fixtureSpanCount)
	}
	resp := ptraceotlp.NewExportResponse()
	if err := resp.UnmarshalJSON(w.Body.Bytes()); err != nil {
		t.Fatalf("response is not a valid ExportTraceServiceResponse: %v", err)
	}

	// An exporter left pointed at the listener root must fail loudly: a 404
	// sends the operator to the endpoint config, where a silent ingest from
	// the wrong path would hide the mistake until the dashboard was empty.
	w = postTo(t, h, "/v1/traces", ctJSON, fixture(t, "pydantic_ai_chat.json"))
	if w.Code != http.StatusNotFound {
		t.Errorf("POST /v1/traces = %d, want 404 (the receiver is mounted at %s)", w.Code, base)
	}
	if got := sink.spans.Load(); got != fixtureSpanCount {
		t.Errorf("root path ingested data: sink spans = %d, want %d", got, fixtureSpanCount)
	}

	// A malformed body under the prefix is a 400, not a 404: proof the request
	// reached the handler instead of being cut off at the mount edge.
	w = postTo(t, h, base+"/v1/traces", ctJSON, []byte(`{"resourceSpans": [{]`))
	if w.Code != http.StatusBadRequest {
		t.Errorf("malformed body under %s = %d, want 400", base, w.Code)
	}
}

// TestOTLPReceiverRootUnchanged: NewHandler is NewHandlerAt(""), so the default
// single-application deployment keeps its previous routing exactly — and the
// prefixed path, which means nothing there, is a 404.
func TestOTLPReceiverRootUnchanged(t *testing.T) {
	sink := &countSink{}
	h := NewHandler(sink, openResolver, 0, 0)

	w := postTo(t, h, "/v1/traces", ctJSON, fixture(t, "pydantic_ai_chat.json"))
	if w.Code != http.StatusOK {
		t.Fatalf("POST /v1/traces at root = %d, body %s", w.Code, w.Body.String())
	}
	if got := sink.spans.Load(); got != fixtureSpanCount {
		t.Fatalf("sink got %d spans, want %d", got, fixtureSpanCount)
	}

	if w := postTo(t, h, "/otterscope/v1/traces", ctJSON, fixture(t, "pydantic_ai_chat.json")); w.Code != http.StatusNotFound {
		t.Errorf("POST /otterscope/v1/traces at a root mount = %d, want 404", w.Code)
	}
}

// TestOTLPReceiverBaseSpellings: NewHandlerAt normalizes the value it is given,
// so a config file spelling ("otterscope/") cannot mount the receiver nowhere
// — a miss there is a receiver that accepts nothing at all.
func TestOTLPReceiverBaseSpellings(t *testing.T) {
	for _, base := range []string{"otterscope", "/otterscope/", "  /otterscope  "} {
		sink := &countSink{}
		w := postTo(t, NewHandlerAt(base, sink, openResolver, 0, 0), "/otterscope/v1/traces", ctJSON, fixture(t, "pydantic_ai_chat.json"))
		if w.Code != http.StatusOK {
			t.Errorf("base %q: POST /otterscope/v1/traces = %d, want 200", base, w.Code)
			continue
		}
		if got := sink.spans.Load(); got != fixtureSpanCount {
			t.Errorf("base %q: sink got %d spans, want %d", base, got, fixtureSpanCount)
		}
	}
}
