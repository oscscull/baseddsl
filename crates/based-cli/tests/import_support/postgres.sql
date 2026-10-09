CREATE SCHEMA based_import_fixture;
CREATE TABLE based_import_fixture.legacy_account (
  account_code TEXT NOT NULL PRIMARY KEY,
  label TEXT NOT NULL DEFAULT 'account'
);
CREATE TABLE based_import_fixture.legacy_entry (
  entry_key BIGINT NOT NULL PRIMARY KEY,
  parent_code TEXT NOT NULL REFERENCES based_import_fixture.legacy_account(account_code) ON DELETE RESTRICT ON UPDATE CASCADE,
  heading TEXT NOT NULL DEFAULT 'original'
);
CREATE INDEX original_parent_lookup ON based_import_fixture.legacy_entry(parent_code);
INSERT INTO based_import_fixture.legacy_account VALUES ('alpha','retained account');
INSERT INTO based_import_fixture.legacy_entry VALUES (41,'alpha','retained original row');
CREATE ROLE based_import_metadata LOGIN PASSWORD 'fixture_metadata_only';
CREATE ROLE based_import_consumer LOGIN PASSWORD 'fixture_select_only';
GRANT USAGE ON SCHEMA based_import_fixture TO based_import_metadata, based_import_consumer;
GRANT SELECT ON ALL TABLES IN SCHEMA based_import_fixture TO based_import_consumer;
