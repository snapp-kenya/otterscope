package server

import (
	"encoding/json"
	"net/http"
)

// problem is an RFC 9457 problem document. Most of this API answers a refusal
// with {"error": "..."}, which a browser can show but a caller cannot act on.
// When the answer is "your request is missing something", the response is a
// problem document instead: Detail says what to fix and Missing names the
// parameters that would have bounded the call.
type problem struct {
	Type     string   `json:"type"`
	Title    string   `json:"title"`
	Status   int      `json:"status"`
	Detail   string   `json:"detail"`
	Instance string   `json:"instance,omitempty"`
	Missing  []string `json:"missing,omitempty"`
}

// writeProblem writes an RFC 9457 problem document with the matching content
// type. Type is the RFC's "about:blank" default: these are generic client
// errors, and a private type URI nobody dereferences is worse than none.
func writeProblem(w http.ResponseWriter, r *http.Request, status int, title, detail string, missing ...string) {
	w.Header().Set("Content-Type", "application/problem+json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(problem{
		Type:     "about:blank",
		Title:    title,
		Status:   status,
		Detail:   detail,
		Instance: r.URL.Path,
		Missing:  missing,
	})
}
