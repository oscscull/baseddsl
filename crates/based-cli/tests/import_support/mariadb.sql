CREATE DATABASE based_import_fixture;
CREATE TABLE based_import_fixture.legacy_account (
  account_code VARCHAR(64) NOT NULL PRIMARY KEY,
  label VARCHAR(128) NOT NULL DEFAULT 'account'
) ENGINE=InnoDB;
CREATE TABLE based_import_fixture.legacy_entry (
  entry_key BIGINT NOT NULL PRIMARY KEY,
  parent_code VARCHAR(64) NOT NULL,
  heading VARCHAR(128) NOT NULL DEFAULT 'original',
  adjustment BIGINT DEFAULT -5,
  ratio DECIMAL(6,2) DEFAULT -1.25,
  CONSTRAINT original_parent FOREIGN KEY (parent_code) REFERENCES based_import_fixture.legacy_account(account_code) ON DELETE RESTRICT ON UPDATE CASCADE
) ENGINE=InnoDB;
CREATE INDEX original_parent_lookup ON based_import_fixture.legacy_entry(parent_code);
INSERT INTO based_import_fixture.legacy_account VALUES ('alpha','retained account');
INSERT INTO based_import_fixture.legacy_entry(entry_key, parent_code, heading) VALUES (41,'alpha','retained original row');
CREATE USER based_import_metadata@'%' IDENTIFIED BY 'fixture_metadata_only';
CREATE USER based_import_consumer@'%' IDENTIFIED BY 'fixture_select_only';
GRANT REFERENCES ON based_import_fixture.* TO based_import_metadata@'%';
GRANT SELECT ON based_import_fixture.* TO based_import_consumer@'%';
