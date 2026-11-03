package server

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/ingest"
	"github.com/otterscope/otterscope/internal/pricing"
	"github.com/otterscope/otterscope/internal/store"
)

// postEvaluate runs one on-demand evaluate call and returns the recorder.
func postEvaluate(t *testing.T, srv *Server, query string) *httptest.ResponseRecorder {
	t.Helper()
	req := newReq(http.MethodPost, "/api/assertions/evaluate"+query, nil)
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, req)
	return w
}

// evalReport is the wire shape the handler returns, decoded independently of
// the ingest type so a renamed JSON field fails the test.
type evalReport struct {
	RunsEvaluated     int `json:"runsEvaluated"`
	RunsSkipped       int `json:"runsSkipped"`
	RunLimit          int `json:"runLimit"`
	JudgeCalls        int `json:"judgeCalls"`
	JudgeCallsSkipped int `json:"judgeCallsSkipped"`
	JudgeCallLimit    int `json:"judgeCallLimit"`
}

func decodeEvalReport(t *testing.T, w *httptest.ResponseRecorder) evalReport {
	t.Helper()
	var rep evalReport
	if err := json.Unmarshal(w.Body.Bytes(), &rep); err != nil {
		t.Fatalf("bad JSON: %v (%s)", err, w.Body.String())
	}
	return rep
}

// decodeProblem decodes an RFC 9457 refusal.
func decodeProblem(t *testing.T, w *httptest.ResponseRecorder) problem {
	t.Helper()
	if ct := w.Header().Get("Content-Type"); ct != "application/problem+json" {
		t.Fatalf("content type = %q, want application/problem+json", ct)
	}
	var p problem
	if err := json.Unmarshal(w.Body.Bytes(), &p); err != nil {
		t.Fatalf("problem document is not JSON: %v (%s)", err, w.Body.String())
	}
	if p.Status != w.Code {
		t.Errorf("problem status = %d, response status = %d", p.Status, w.Code)
	}
	return p
}

// A call that names nothing scores every run in the project — the unbounded
// cost bomb. It must be refused before any work, with a body that says what
// to add.
func TestEvaluateRefusesUnboundedCall(t *testing.T) {
	srv, st := testServer(t)
	seedRun(t, st, "r1", 1000)

	w := postEvaluate(t, srv, "?project=default")
	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400 (%s)", w.Code, w.Body.String())
	}
	p := decodeProblem(t, w)
	for _, want := range []string{"run", "since", "limit"} {
		if !strings.Contains(p.Detail, want) {
			t.Errorf("detail %q does not name %q", p.Detail, want)
		}
	}
	if len(p.Missing) == 0 {
		t.Fatal("problem document does not say which parameter is missing")
	}
	if p.Title != "evaluate needs a bound" {
		t.Errorf("title = %q", p.Title)
	}
}

// A bounded call does the work it was asked for: by run id, and by window.
func TestEvaluateBoundedCallProcessed(t *testing.T) {
	srv, st := testServer(t)
	seedRun(t, st, "r1", 1000)
	seedRun(t, st, "r2", 2000)

	mk := jsonReq(http.MethodPost, "/api/assertions",
		`{"name":"fast","type":"max_latency_ms","config":"20000"}`)
	mw := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(mw, mk)
	if mw.Code != http.StatusCreated {
		t.Fatalf("create assertion: %d %s", mw.Code, mw.Body.String())
	}

	w := postEvaluate(t, srv, "?run=r1")
	if w.Code != http.StatusOK {
		t.Fatalf("run-bound evaluate: %d %s", w.Code, w.Body.String())
	}
	if rep := decodeEvalReport(t, w); rep.RunsEvaluated != 1 || rep.RunsSkipped != 0 {
		t.Fatalf("run-bound report = %+v, want 1 evaluated and no skips", rep)
	}

	// A window is the other accepted bound, and it scores both runs.
	w = postEvaluate(t, srv, "?since=1970-01-01T00:00:00Z&until=2000-01-01T00:00:00Z")
	if w.Code != http.StatusOK {
		t.Fatalf("window-bound evaluate: %d %s", w.Code, w.Body.String())
	}
	if rep := decodeEvalReport(t, w); rep.RunsEvaluated != 2 {
		t.Fatalf("window report = %+v, want 2 evaluated", rep)
	}
}

// Past the run cap the call stops and reports the remainder rather than
// silently evaluating a subset (a truncated backfill reads as complete).
func TestEvaluateCappedReportsSkips(t *testing.T) {
	srv, st := testServer(t)
	for i, id := range []string{"r1", "r2", "r3", "r4", "r5"} {
		seedRun(t, st, id, int64(1000+i))
	}

	old := ingest.MaxEvaluateRuns
	ingest.MaxEvaluateRuns = 2
	t.Cleanup(func() { ingest.MaxEvaluateRuns = old })

	w := postEvaluate(t, srv, "?project=default&limit=10")
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, %s", w.Code, w.Body.String())
	}
	rep := decodeEvalReport(t, w)
	if rep.RunsEvaluated != 2 {
		t.Errorf("runsEvaluated = %d, want 2 (the cap)", rep.RunsEvaluated)
	}
	if rep.RunsSkipped != 3 {
		t.Errorf("runsSkipped = %d, want 3", rep.RunsSkipped)
	}
	if rep.RunLimit != 2 {
		t.Errorf("runLimit = %d, want the server cap 2", rep.RunLimit)
	}
}

// A caller-supplied limit is a bound and also lowers the cap; it can never
// raise it past the server maximum.
func TestEvaluateCallerLimitCannotExceedCap(t *testing.T) {
	srv, st := testServer(t)
	for i, id := range []string{"r1", "r2", "r3"} {
		seedRun(t, st, id, int64(1000+i))
	}

	old := ingest.MaxEvaluateRuns
	ingest.MaxEvaluateRuns = 2
	t.Cleanup(func() { ingest.MaxEvaluateRuns = old })

	w := postEvaluate(t, srv, "?limit=1")
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, %s", w.Code, w.Body.String())
	}
	if rep := decodeEvalReport(t, w); rep.RunsEvaluated != 1 || rep.RunsSkipped != 2 {
		t.Fatalf("report = %+v, want 1 evaluated, 2 skipped", rep)
	}

	// ?limit=99 is clamped to the server cap, not honoured as 99.
	w = postEvaluate(t, srv, "?limit=99")
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, %s", w.Code, w.Body.String())
	}
	if rep := decodeEvalReport(t, w); rep.RunsEvaluated != 2 || rep.RunsSkipped != 1 {
		t.Fatalf("report = %+v, want 2 evaluated (clamped), 1 skipped", rep)
	}
}

// judgeServer builds a server whose judge endpoint has a key and a one-model
// allow-list.
func judgeServer(t *testing.T, models []string) (*Server, *store.Store) {
	t.Helper()
	st, err := store.Open(context.Background(), t.TempDir()+"/test.db")
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { st.Close() })
	ep := evals.Endpoint{BaseURL: "http://unused.invalid", Key: "sk-test", Models: models}
	return New(st, pricing.Default(), ep, 0, false, ingest.Options{}, "", "test"), st
}

// An assertion created before the allow-list existed (or by any other path)
// must not be evaluated: it would bill the model it names.
func TestEvaluateRefusesDisallowedJudgeModel(t *testing.T) {
	srv, st := judgeServer(t, []string{"gpt-4o-mini"})
	seedRun(t, st, "r1", 1000)
	if _, err := st.CreateAssertion(context.Background(), evals.Assertion{
		Name: "helpful", Type: "llm_judge", Enabled: true,
		Config: `{"prompt":"is it helpful?","model":"priciest-model-available"}`,
	}); err != nil {
		t.Fatalf("seed assertion: %v", err)
	}

	w := postEvaluate(t, srv, "?run=r1")
	if w.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400 (%s)", w.Code, w.Body.String())
	}
	p := decodeProblem(t, w)
	if !strings.Contains(p.Detail, "priciest-model-available") {
		t.Errorf("detail %q does not name the refused model", p.Detail)
	}
	if !strings.Contains(p.Detail, "gpt-4o-mini") {
		t.Errorf("detail %q does not name the allow-list", p.Detail)
	}
}

// Creating an assertion that names a model outside the allow-list is refused
// at the source, with the model named.
func TestCreateAssertionRefusesDisallowedJudgeModel(t *testing.T) {
	srv, _ := judgeServer(t, []string{"gpt-4o-mini"})

	mw := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(mw, jsonReq(http.MethodPost, "/api/assertions",
		`{"name":"helpful","type":"llm_judge","config":"{\"prompt\":\"ok?\",\"model\":\"priciest-model-available\"}"}`))
	if mw.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400 (%s)", mw.Code, mw.Body.String())
	}
	if p := decodeProblem(t, mw); !strings.Contains(p.Detail, "priciest-model-available") {
		t.Fatalf("detail %q does not name the refused model", p.Detail)
	}

	// The allowed model still creates.
	mw = httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(mw, jsonReq(http.MethodPost, "/api/assertions",
		`{"name":"helpful","type":"llm_judge","config":"{\"prompt\":\"ok?\",\"model\":\"gpt-4o-mini\"}"}`))
	if mw.Code != http.StatusCreated {
		t.Fatalf("allowed model refused: %d %s", mw.Code, mw.Body.String())
	}
}

// The judge-call cap is separate from the run cap: three runs with two judge
// assertions each is six paid calls, and a cap of zero must make none of
// them, reporting all six as skipped.
func TestEvaluateJudgeCallCapReported(t *testing.T) {
	srv, st := judgeServer(t, []string{"gpt-4o-mini"})
	for i, id := range []string{"r1", "r2", "r3"} {
		seedRun(t, st, id, int64(1000+i))
	}
	for _, name := range []string{"helpful", "concise"} {
		if _, err := st.CreateAssertion(context.Background(), evals.Assertion{
			Name: name, Type: "llm_judge", Enabled: true,
			Config: `{"prompt":"judge it","model":"gpt-4o-mini"}`,
		}); err != nil {
			t.Fatalf("seed assertion %s: %v", name, err)
		}
	}

	old := ingest.MaxEvaluateJudgeCalls
	ingest.MaxEvaluateJudgeCalls = 0
	t.Cleanup(func() { ingest.MaxEvaluateJudgeCalls = old })

	w := postEvaluate(t, srv, "?limit=50")
	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, %s", w.Code, w.Body.String())
	}
	rep := decodeEvalReport(t, w)
	if rep.JudgeCalls != 0 {
		t.Errorf("judgeCalls = %d, want 0 under a zero cap", rep.JudgeCalls)
	}
	if rep.JudgeCallsSkipped != 6 {
		t.Errorf("judgeCallsSkipped = %d, want 6 (3 runs x 2 judge assertions)", rep.JudgeCallsSkipped)
	}
	if rep.JudgeCallLimit != 0 {
		t.Errorf("judgeCallLimit = %d, want 0", rep.JudgeCallLimit)
	}
}

// An unknown run id is a 404, not a 500: the request was well formed.
func TestEvaluateUnknownRunIsNotFound(t *testing.T) {
	srv, _ := testServer(t)
	w := postEvaluate(t, srv, "?run=nope")
	if w.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404 (%s)", w.Code, w.Body.String())
	}
}

// A malformed bound is refused as a client error rather than silently
// ignored, which would turn a typo into an unbounded sweep.
func TestEvaluateRejectsMalformedBounds(t *testing.T) {
	srv, st := testServer(t)
	seedRun(t, st, "r1", 1000)

	for _, q := range []string{"?since=yesterday", "?limit=0", "?limit=lots"} {
		w := postEvaluate(t, srv, q)
		if w.Code != http.StatusBadRequest {
			t.Errorf("%s: status = %d, want 400 (%s)", q, w.Code, w.Body.String())
		}
	}
}
