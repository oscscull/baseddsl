CREATE TABLE legacy_account (
  account_code TEXT NOT NULL PRIMARY KEY,
  label TEXT NOT NULL DEFAULT 'account'
);
CREATE TABLE legacy_entry (
  entry_key BIGINT NOT NULL PRIMARY KEY,
  parent_code TEXT NOT NULL REFERENCES legacy_account(account_code) ON DELETE RESTRICT ON UPDATE CASCADE,
  heading TEXT NOT NULL DEFAULT 'original'
);
CREATE INDEX original_parent_lookup ON legacy_entry(parent_code);
INSERT INTO legacy_account VALUES ('alpha', 'retained account');
INSERT INTO legacy_entry VALUES (41, 'alpha', 'retained original row');
