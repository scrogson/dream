package tree_sitter_toybeam_test

import (
	"testing"

	tree_sitter "github.com/smacker/go-tree-sitter"
	"github.com/tree-sitter/tree-sitter-toybeam"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_toybeam.Language())
	if language == nil {
		t.Errorf("Error loading Toybeam grammar")
	}
}
