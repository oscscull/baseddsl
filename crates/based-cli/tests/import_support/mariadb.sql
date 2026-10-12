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
CREATE TABLE based_import_fixture.legacy_ledger (
  region VARCHAR(64) NOT NULL,
  ledger_no BIGINT NOT NULL,
  title VARCHAR(128) NOT NULL,
  PRIMARY KEY (region, ledger_no)
) ENGINE=InnoDB;
CREATE TABLE based_import_fixture.legacy_posting (
  posting_key BIGINT NOT NULL PRIMARY KEY,
  memo VARCHAR(128) NOT NULL,
  post_ledger BIGINT NOT NULL,
  `post region` VARCHAR(64) NOT NULL,
  CONSTRAINT posting_ledger FOREIGN KEY (`post region`, post_ledger) REFERENCES based_import_fixture.legacy_ledger(region, ledger_no)
) ENGINE=InnoDB;
INSERT INTO based_import_fixture.legacy_account VALUES ('alpha','retained account');
INSERT INTO based_import_fixture.legacy_entry(entry_key, parent_code, heading) VALUES (41,'alpha','retained original row');
INSERT INTO based_import_fixture.legacy_ledger VALUES ('north',7,'retained ledger');
INSERT INTO based_import_fixture.legacy_posting VALUES (3,'retained posting',7,'north');
CREATE USER based_import_metadata@'%' IDENTIFIED BY 'fixture_metadata_only';
CREATE USER based_import_consumer@'%' IDENTIFIED BY 'fixture_select_only';
GRANT REFERENCES ON based_import_fixture.* TO based_import_metadata@'%';
GRANT SELECT ON based_import_fixture.* TO based_import_consumer@'%';
