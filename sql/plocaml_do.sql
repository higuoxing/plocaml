DO $$ PL.notice "This is plocaml." $$ LANGUAGE plocaml;

DO $$ failwith "error test" $$ LANGUAGE plocaml;
