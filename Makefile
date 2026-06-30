EXTENSION = plocaml
DATA = plocaml--1.0.sql
MODULES = plocaml
REGRESS = plocaml_call plocaml_spi

PG_CONFIG = pg_config
PGXS := $(shell $(PG_CONFIG) --pgxs)
include $(PGXS)

plocaml$(DLSUFFIX): src/runtime.ml src/stub.c
	dune build
	cp _build/default/src/runtime.bc.so $@

clean-dune:
	dune clean
clean: clean-dune
