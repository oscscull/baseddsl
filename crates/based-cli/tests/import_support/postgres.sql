CREATE SCHEMA based_import_fixture;
CREATE TABLE based_import_fixture.legacy_account (
  account_code TEXT NOT NULL PRIMARY KEY,
  label TEXT NOT NULL DEFAULT 'account'
);
CREATE TABLE based_import_fixture.legacy_entry (
  entry_key BIGINT NOT NULL PRIMARY KEY,
  parent_code TEXT NOT NULL REFERENCES based_import_fixture.legacy_account(account_code) ON DELETE RESTRICT ON UPDATE CASCADE,
  heading TEXT NOT NULL DEFAULT 'original',
  adjustment BIGINT DEFAULT -5,
  ratio NUMERIC(6,2) DEFAULT -1.25
);
CREATE INDEX original_parent_lookup ON based_import_fixture.legacy_entry(parent_code);
CREATE TABLE based_import_fixture.legacy_ledger (
  region TEXT NOT NULL,
  ledger_no BIGINT NOT NULL,
  title TEXT NOT NULL,
  PRIMARY KEY (region, ledger_no)
);
CREATE TABLE based_import_fixture.legacy_posting (
  posting_key BIGINT NOT NULL PRIMARY KEY,
  memo TEXT NOT NULL,
  post_ledger BIGINT NOT NULL,
  "post region" TEXT NOT NULL,
  FOREIGN KEY ("post region", post_ledger) REFERENCES based_import_fixture.legacy_ledger(region, ledger_no)
);
INSERT INTO based_import_fixture.legacy_account VALUES ('alpha','retained account');
INSERT INTO based_import_fixture.legacy_entry(entry_key, parent_code, heading) VALUES (41,'alpha','retained original row');
INSERT INTO based_import_fixture.legacy_ledger VALUES ('north',7,'retained ledger');
INSERT INTO based_import_fixture.legacy_posting VALUES (3,'retained posting',7,'north');
CREATE ROLE based_import_metadata LOGIN PASSWORD 'fixture_metadata_only';
CREATE ROLE based_import_consumer LOGIN PASSWORD 'fixture_select_only';
GRANT USAGE ON SCHEMA based_import_fixture TO based_import_metadata, based_import_consumer;
GRANT SELECT ON ALL TABLES IN SCHEMA based_import_fixture TO based_import_consumer;
