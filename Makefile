.PHONY: fmt
fmt:
	cargo fmt
	ocamlformat --inplace ml/*.ml
