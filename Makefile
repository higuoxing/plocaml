EXTENSION = plocaml
DATA = plocaml--1.0.sql
MODULES = plocaml
REGRESS = plocaml_call plocaml_spi plocaml_void plocaml_do plocaml_spi_nested plocaml_error_test plocaml_composite plocaml_drop

PG_CONFIG = pg_config
PGXS := $(shell $(PG_CONFIG) --pgxs)
include $(PGXS)

plocaml$(DLSUFFIX): src/runtime.ml src/stub.c src/bootstrap.ml
	dune build
	cp -f _build/default/src/runtime.bc.so $@

clean-dune:
	dune clean
clean: clean-dune

format:
	clang-format -i src/*.c src/*.h
