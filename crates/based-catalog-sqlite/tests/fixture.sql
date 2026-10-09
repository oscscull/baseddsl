CREATE TABLE "odd"" table" (
 "part z" INTEGER NOT NULL, "part a" INTEGER NOT NULL,
 "text-value" TEXT DEFAULT 'it''s retained', amount REAL DEFAULT 10.125,
 next_z INTEGER, next_a INTEGER,
 PRIMARY KEY("part z","part a"), UNIQUE("part a","part z"),
 CONSTRAINT fk_self FOREIGN KEY(next_z,next_a) REFERENCES "odd"" table"("part z","part a")
) WITHOUT ROWID;
CREATE TABLE cycle_a(id INTEGER PRIMARY KEY, peer INTEGER REFERENCES cycle_b(id));
CREATE TABLE cycle_b(id INTEGER PRIMARY KEY, peer INTEGER REFERENCES cycle_a(id) ON DELETE CASCADE);
CREATE TABLE keyless(label TEXT, guessed_id INTEGER, "CHECK" TEXT DEFAULT 'COLLATE');
CREATE TABLE rowid_auto(id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT);
CREATE TABLE rowid_reuse(id INTEGER PRIMARY KEY, label TEXT);
CREATE TABLE descending_pk(id INTEGER PRIMARY KEY DESC, label TEXT);
CREATE TABLE strict_values(id INT PRIMARY KEY, label TEXT NOT NULL) STRICT;
CREATE TABLE untyped(value, "AUTOINCREMENT" TEXT DEFAULT 'CHECK');
CREATE TABLE native_details(
 id INTEGER PRIMARY KEY, label TEXT COLLATE NOCASE,
 decimal_value DECIMAL(12,3), boolean_value BOOLEAN,
 generated_virtual TEXT GENERATED ALWAYS AS (upper(label)) VIRTUAL,
 generated_stored INTEGER GENERATED ALWAYS AS (length(label)) STORED,
 CONSTRAINT meaningful CHECK (id > 0)
);
CREATE INDEX expression_partial ON native_details(lower(label)) WHERE label IS NOT NULL;
CREATE TABLE foreign_semantics(id INTEGER PRIMARY KEY, peer INTEGER,
 FOREIGN KEY(peer) REFERENCES rowid_reuse DEFERRABLE INITIALLY DEFERRED);
CREATE VIEW selected_view AS SELECT id FROM cycle_a;
INSERT INTO "odd"" table"("part z","part a") VALUES (7,9);
INSERT INTO keyless VALUES('application secret',123,'fixture');
