EXTENSION = plocamlu
DATA = plocamlu--1.0.sql
MODULES = plocamlu
REGRESS = plocaml_call plocaml_spi plocaml_void plocaml_do plocaml_spi_nested plocaml_composite plocaml_gd_sd plocaml_global plocaml_import plocaml_ereport plocaml_error plocaml_drop
REGRESS_OPTS = --load-extension=plocamlu

PG_CONFIG = pg_config
PGXS := $(shell $(PG_CONFIG) --pgxs)
include $(PGXS)

plocamlu$(DLSUFFIX): src/runtime.ml src/stub.c src/bootstrap.ml
	dune build
	cp -f _build/default/src/runtime.bc.so $@

clean-dune:
	dune clean
clean: clean-dune

format:
	clang-format -i src/*.c src/*.h
