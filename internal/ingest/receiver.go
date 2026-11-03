// Package ingest owns the OTLP/HTTP receiver and (in later issues) the
// normalization of GenAI span dialects into internal/model. OpenTelemetry
// types must not be used outside this package.
//
// Spec deviation, deliberate: error response bodies are text/plain instead
// of google.rpc.Status protos. OTLP exporters act on HTTP status codes;
// encoding Status would add a genproto dependency for bodies nothing reads.
// Revisit via an issue if a real SDK misbehaves on it.
package ingest

import (
	"compress/gzip"
	"context"
	"fmt"
	"io"
	"log/slog"
	"mime"
	"net/http"
	"strings"

	"github.com/otterscope/otterscope/internal/basepath"
	"go.opentelemetry.io/collector/pdata/ptrace"
	"go.opentelemetry.io/collector/pdata/ptrace/ptraceotlp"
)

// maxBodyBytes caps decompressed request bodies. 32 MiB is far above any
// sane single OTLP batch.
const maxBodyBytes = 32 << 20

const (
	ctProto = "application/x-protobuf"
	ctJSON  = "application/json"
)

// Result reports what a sink did with one batch.
type Result struct {
	// RejectedSpans counts spans that were not written because their
	// (project, span ID) already exists for a different resource service.
	// Receivers report it to the client via partialSuccess, never silently.
	RejectedSpans int
	// Message explains the rejection in words, for the client's log.
	Message string
}

// Sink receives every decoded trace batch, tagged with the project the
// caller's ingest key resolved to. Implementations must be safe for
// concurrent use.
type Sink interface {
	ConsumeTraces(ctx context.Context, project string, td ptrace.Traces) (Result, error)
}

// KeyResolver maps a Bearer ingest key ("" = none supplied) to a project
// name; ok=false rejects the request.
type KeyResolver func(ctx context.Context, key string) (project string, ok bool)

// Work caps. A single OTLP request must not be able to monopolise the sole
// SQLite writer: a 5 MB body can otherwise carry tens of thousands of distinct
// trace IDs and stall every concurrent reader for over a minute.
const (
	// DefaultMaxSpansPerBatch is the most spans one request may carry.
	DefaultMaxSpansPerBatch = 10000
	// DefaultMaxTraceIDsPerBatch is the most distinct trace IDs (runs) one
	// request may touch, which bounds the run re-derivation work.
	DefaultMaxTraceIDsPerBatch = 1000
)

// Options configures the OTLP receiver.
type Options struct {
	// BasePath mounts the receiver under a path prefix ("" = listener root).
	BasePath string

	// RatePerSec and Burst set the per-project ingest rate limit. 0 means
	// unlimited (deliberately opt-out); a non-positive burst means 2x rate.
	RatePerSec, Burst float64

	// RequireKey rejects requests that present no ingest key. Set it whenever
	// read auth is on: otherwise an unauthenticated sender still lands in the
	// "default" project and can rewrite its steps.
	RequireKey bool

	// MaxSpans and MaxTraceIDs cap per-request work; 0 means the package
	// default. Excess batches are refused whole, before any work is done.
	MaxSpans    int
	MaxTraceIDs int
}

func (o Options) maxSpans() int {
	if o.MaxSpans > 0 {
		return o.MaxSpans
	}
	return DefaultMaxSpansPerBatch
}

func (o Options) maxTraceIDs() int {
	if o.MaxTraceIDs > 0 {
		return o.MaxTraceIDs
	}
	return DefaultMaxTraceIDsPerBatch
}

// NewHandler returns the OTLP/HTTP handler at the listener root, routing
// POST /v1/traces to sink. ratePerSec/burst set an optional per-key ingest rate
// limit (0 = unlimited).
func NewHandler(sink Sink, resolve KeyResolver, ratePerSec, burst float64) http.Handler {
	return NewHandlerWithOptions(sink, resolve, Options{RatePerSec: ratePerSec, Burst: burst})
}

// NewHandlerAt is NewHandler mounted under base ("" = listener root). OTLP/HTTP
// exporters append /v1/traces to the endpoint they are configured with, so an
// exporter pointed at https://host/otterscope posts to
// https://host/otterscope/v1/traces — the prefix must reach the receiver too.
//
// base is normalized here ("otterscope/", "/otterscope" and "/" all work), so a
// caller cannot silently mount nothing by passing a trailing slash. A base that
// cannot be normalized at all mounts nothing and answers 404, matching
// basepath.Strip's behaviour for a path outside the mount.
func NewHandlerAt(base string, sink Sink, resolve KeyResolver, ratePerSec, burst float64) http.Handler {
	o := Options{RatePerSec: ratePerSec, Burst: burst}
	o.BasePath = base
	return NewHandlerWithOptions(sink, resolve, o)
}

// NewHandlerWithOptions builds the receiver from o.
func NewHandlerWithOptions(sink Sink, resolve KeyResolver, o Options) http.Handler {
	base := o.BasePath
	if normalized, err := basepath.Normalize(base); err == nil {
		base = normalized
	}
	mux := http.NewServeMux()
	mux.Handle("POST /v1/traces", &tracesHandler{
		sink:        sink,
		resolve:     resolve,
		limit:       newRateLimiter(o.RatePerSec, o.Burst),
		requireKey:  o.RequireKey,
		maxSpans:    o.maxSpans(),
		maxTraceIDs: o.maxTraceIDs(),
	})
	return basepath.Strip(base, mux)
}

type tracesHandler struct {
	sink        Sink
	resolve     KeyResolver
	limit       *rateLimiter
	requireKey  bool
	maxSpans    int
	maxTraceIDs int
}

func (h *tracesHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	key := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
	if h.requireKey && key == "" {
		http.Error(w, "ingest key required", http.StatusUnauthorized)
		return
	}
	project, ok := h.resolve(r.Context(), key)
	if !ok {
		http.Error(w, "unknown ingest key", http.StatusUnauthorized)
		return
	}
	// Rate-limit per project before doing any work.
	if !h.limit.allow(project) {
		http.Error(w, "ingest rate limit exceeded", http.StatusTooManyRequests)
		return
	}

	contentType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || (contentType != ctProto && contentType != ctJSON) {
		http.Error(w, "unsupported content type: use application/x-protobuf or application/json", http.StatusUnsupportedMediaType)
		return
	}

	body, err := readBody(r)
	if err != nil {
		status := http.StatusBadRequest
		if _, ok := err.(*http.MaxBytesError); ok {
			status = http.StatusRequestEntityTooLarge
		}
		http.Error(w, err.Error(), status)
		return
	}

	req := ptraceotlp.NewExportRequest()
	switch contentType {
	case ctProto:
		err = req.UnmarshalProto(body)
	case ctJSON:
		err = req.UnmarshalJSON(body)
	}
	if err != nil {
		http.Error(w, fmt.Sprintf("malformed %s payload: %v", contentType, err), http.StatusBadRequest)
		return
	}
	td := req.Traces()

	// Refuse an oversized batch before any normalization or database work:
	// the cost of a request must be bounded by these caps, not by its body.
	if n := td.SpanCount(); n > h.maxSpans {
		http.Error(w, fmt.Sprintf(
			"batch rejected: %d spans exceeds the maxSpansPerBatch limit of %d", n, h.maxSpans),
			http.StatusBadRequest)
		return
	}
	if n := countTraceIDs(td, h.maxTraceIDs); n > h.maxTraceIDs {
		http.Error(w, fmt.Sprintf(
			"batch rejected: more than maxTraceIdsPerBatch distinct trace ids (limit %d)", h.maxTraceIDs),
			http.StatusBadRequest)
		return
	}

	res, err := h.sink.ConsumeTraces(r.Context(), project, td)
	if err != nil {
		slog.Error("ingest: sink failed", "err", err)
		http.Error(w, "failed to store traces", http.StatusInternalServerError)
		return
	}

	writeExportResponse(w, contentType, res)
}

// countTraceIDs counts the distinct trace IDs in td, stopping as soon as it
// exceeds limit so a hostile batch cannot make the check itself expensive.
func countTraceIDs(td ptrace.Traces, limit int) int {
	ids := make(map[[16]byte]struct{})
	rss := td.ResourceSpans()
	for i := 0; i < rss.Len(); i++ {
		sss := rss.At(i).ScopeSpans()
		for j := 0; j < sss.Len(); j++ {
			spans := sss.At(j).Spans()
			for k := 0; k < spans.Len(); k++ {
				tid := spans.At(k).TraceID()
				if tid.IsEmpty() {
					continue
				}
				ids[[16]byte(tid)] = struct{}{}
				if len(ids) > limit {
					return len(ids)
				}
			}
		}
	}
	return len(ids)
}

func readBody(r *http.Request) ([]byte, error) {
	var reader io.Reader = http.MaxBytesReader(nil, r.Body, maxBodyBytes)
	if r.Header.Get("Content-Encoding") == "gzip" {
		gz, err := gzip.NewReader(reader)
		if err != nil {
			return nil, fmt.Errorf("bad gzip body: %w", err)
		}
		defer gz.Close()
		// Cap the decompressed size too, or a tiny gzip bomb bypasses the
		// body limit.
		reader = io.LimitReader(gz, maxBodyBytes+1)
	}
	body, err := io.ReadAll(reader)
	if err != nil {
		return nil, err
	}
	if len(body) > maxBodyBytes {
		return nil, &http.MaxBytesError{Limit: maxBodyBytes}
	}
	return body, nil
}

// writeExportResponse encodes the OTLP export response, reporting refused
// spans in partialSuccess so the client sees them instead of assuming success.
func writeExportResponse(w http.ResponseWriter, contentType string, res Result) {
	resp := ptraceotlp.NewExportResponse()
	if res.RejectedSpans > 0 {
		resp.PartialSuccess().SetRejectedSpans(int64(res.RejectedSpans))
		resp.PartialSuccess().SetErrorMessage(res.Message)
	}
	var body []byte
	var err error
	switch contentType {
	case ctProto:
		body, err = resp.MarshalProto()
	case ctJSON:
		body, err = resp.MarshalJSON()
	}
	if err != nil {
		http.Error(w, "failed to encode response", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", contentType)
	w.Write(body)
}
