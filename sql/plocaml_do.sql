DO $$ PL.notice "This is plocamlu." $$ LANGUAGE plocamlu;

DO $$ failwith "error test" $$ LANGUAGE plocamlu;
