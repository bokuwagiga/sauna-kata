package harness

import (
	"encoding/json"
	"testing"
)

// The schema names one required property per criterion, carries the
// criterion text, and asks for reason before pass.
func TestVerdictSchema(t *testing.T) {
	raw := verdictSchema([]string{"asks for the name", "does not book"})
	var s struct {
		Type       string `json:"type"`
		Required   []string
		Properties map[string]struct {
			Description string `json:"description"`
			Required    []string
			Properties  map[string]any
		}
		AdditionalProperties bool
	}
	if err := json.Unmarshal(raw, &s); err != nil {
		t.Fatalf("schema is not valid JSON: %v\n%s", err, raw)
	}
	if len(s.Required) != 2 || s.Required[0] != "criterion_1" || s.Required[1] != "criterion_2" {
		t.Errorf("required = %v", s.Required)
	}
	c2 := s.Properties["criterion_2"]
	if c2.Description != "does not book" {
		t.Errorf("criterion_2 description = %q", c2.Description)
	}
	if len(c2.Required) != 2 || c2.Required[0] != "reason" || c2.Required[1] != "pass" {
		t.Errorf("verdict required = %v, want reason then pass", c2.Required)
	}
	if s.AdditionalProperties {
		t.Error("additionalProperties must be false for strict mode")
	}
}
