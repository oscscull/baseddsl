CREATE SCHEMA "catalog fixture";
CREATE SCHEMA catalog_other;
CREATE TYPE "catalog fixture".mood AS ENUM ('second', 'first');
CREATE DOMAIN "catalog fixture".positive_number AS INTEGER DEFAULT 1 NOT NULL CHECK(VALUE > 0);
CREATE TABLE "catalog fixture"."odd"" table" (
 "part z" INTEGER NOT NULL, "part a" INTEGER NOT NULL,
 "text-value" VARCHAR(80) COLLATE "C" DEFAULT 'it''s retained',
 amount NUMERIC(12,3) DEFAULT 10.125, next_z INTEGER, next_a INTEGER,
 PRIMARY KEY("part z","part a"), UNIQUE("part a","part z"),
 CONSTRAINT fk_self FOREIGN KEY(next_z,next_a) REFERENCES "catalog fixture"."odd"" table"("part z","part a")
);
CREATE TABLE "catalog fixture".cycle_a(id BIGINT PRIMARY KEY, peer BIGINT);
CREATE TABLE catalog_other.cycle_b(id BIGINT PRIMARY KEY, peer BIGINT,
 CONSTRAINT fk_to_a FOREIGN KEY(peer) REFERENCES "catalog fixture".cycle_a(id) ON DELETE CASCADE);
ALTER TABLE "catalog fixture".cycle_a ADD CONSTRAINT fk_to_b FOREIGN KEY(peer) REFERENCES catalog_other.cycle_b(id);
CREATE TABLE "catalog fixture".keyless(label TEXT, guessed_id INTEGER);
CREATE TABLE "catalog fixture".native_details (
 id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 sequence_id BIGSERIAL,
 enum_value "catalog fixture".mood,
 domain_value "catalog fixture".positive_number,
 array_value INTEGER[],
 computed INTEGER GENERATED ALWAYS AS (sequence_id + 1) STORED,
 label TEXT, fractional NUMERIC(5,-2), zoned TIMESTAMPTZ(3),
 CONSTRAINT nonnegative CHECK (sequence_id >= 0)
);
CREATE INDEX expression_partial ON "catalog fixture".native_details(lower(label)) INCLUDE(sequence_id) WHERE label IS NOT NULL;
CREATE INDEX nondefault_order ON "catalog fixture".native_details(label NULLS FIRST);
CREATE UNIQUE INDEX nulls_equal ON "catalog fixture".native_details(label) NULLS NOT DISTINCT;
CREATE TABLE "catalog fixture".foreign_semantics (
 a INTEGER, b INTEGER, x INTEGER DEFAULT 7, y INTEGER DEFAULT 9,
 PRIMARY KEY(a,b) DEFERRABLE INITIALLY DEFERRED,
 CONSTRAINT special_fk FOREIGN KEY(x,y) REFERENCES "catalog fixture"."odd"" table"("part z","part a")
 MATCH FULL ON DELETE SET DEFAULT DEFERRABLE INITIALLY DEFERRED
);
ALTER TABLE "catalog fixture".foreign_semantics ADD CONSTRAINT unvalidated CHECK(a >= 0) NOT VALID;
ALTER TABLE "catalog fixture".native_details ENABLE ROW LEVEL SECURITY;
CREATE SCHEMA catalog_hidden;
CREATE TABLE catalog_hidden.private_table(id INTEGER PRIMARY KEY);
CREATE VIEW "catalog fixture".selected_view AS SELECT id FROM "catalog fixture".cycle_a;
INSERT INTO "catalog fixture"."odd"" table"("part z","part a") VALUES(7,9);
INSERT INTO "catalog fixture".keyless VALUES('application secret',123);
CREATE USER based_catalog_reader PASSWORD 'fixture_metadata_only';
GRANT USAGE ON SCHEMA "catalog fixture", catalog_other TO based_catalog_reader;
