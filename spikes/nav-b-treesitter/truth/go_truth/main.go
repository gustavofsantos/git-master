// Ground truth for Go: the declarations go/parser finds.
// usage: go run . <root> > truth.tsv      rows: path<TAB>line<TAB>class<TAB>name
package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

func main() {
	root := os.Args[1]
	var rows []string
	filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if d.Name() == ".git" {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(p, ".go") {
			return nil
		}
		if info, e := d.Info(); e == nil && info.Size() > 2*1024*1024 {
			return nil
		}
		fset := token.NewFileSet()
		f, e := parser.ParseFile(fset, p, nil, 0)
		if e != nil && f == nil {
			return nil
		}
		rel, _ := filepath.Rel(root, p)
		ast.Inspect(f, func(n ast.Node) bool {
			switch x := n.(type) {
			case *ast.FuncDecl:
				rows = append(rows, fmt.Sprintf("%s\t%d\tfn\t%s", rel, fset.Position(x.Name.Pos()).Line, x.Name.Name))
			case *ast.TypeSpec:
				rows = append(rows, fmt.Sprintf("%s\t%d\ttype\t%s", rel, fset.Position(x.Name.Pos()).Line, x.Name.Name))
			}
			return true
		})
		return nil
	})
	sort.Strings(rows)
	fmt.Println(strings.Join(rows, "\n"))
}
