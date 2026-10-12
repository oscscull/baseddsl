CREATE TABLE legacy_account (
  account_code TEXT NOT NULL PRIMARY KEY,
  label TEXT NOT NULL DEFAULT 'account'
);
CREATE TABLE legacy_entry (
  entry_key BIGINT NOT NULL PRIMARY KEY,
  parent_code TEXT NOT NULL REFERENCES legacy_account(account_code) ON DELETE RESTRICT ON UPDATE CASCADE,
  heading TEXT NOT NULL DEFAULT 'original',
  adjustment INTEGER DEFAULT -5,
  ratio REAL DEFAULT -1.25
);
CREATE INDEX original_parent_lookup ON legacy_entry(parent_code);
CREATE TABLE legacy_ledger (
  region TEXT NOT NULL,
  ledger_no BIGINT NOT NULL,
  title TEXT NOT NULL,
  PRIMARY KEY (region, ledger_no)
);
CREATE TABLE legacy_posting (
  posting_key BIGINT NOT NULL PRIMARY KEY,
  memo TEXT NOT NULL,
  post_ledger BIGINT NOT NULL,
  "post region" TEXT NOT NULL,
  FOREIGN KEY ("post region", post_ledger) REFERENCES legacy_ledger(region, ledger_no)
);
INSERT INTO legacy_account VALUES ('alpha', 'retained account');
INSERT INTO legacy_entry(entry_key, parent_code, heading) VALUES (41, 'alpha', 'retained original row');
INSERT INTO legacy_ledger VALUES ('north', 7, 'retained ledger');
INSERT INTO legacy_posting VALUES (3, 'retained posting', 7, 'north');
