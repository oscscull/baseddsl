CREATE DATABASE based_catalog_fixture;
CREATE TABLE based_catalog_fixture.`odd`` table` (
  `part z` INT NOT NULL,
  `part a` INT NOT NULL,
  `text-value` VARCHAR(80) COLLATE utf8mb4_bin DEFAULT 'it''s retained',
  amount DECIMAL(12,3) DEFAULT 10.125,
  next_z INT, next_a INT,
  PRIMARY KEY (`part z`, `part a`),
  UNIQUE KEY uq_parts (`part a`, `part z`),
  CONSTRAINT fk_self FOREIGN KEY (next_z,next_a) REFERENCES based_catalog_fixture.`odd`` table` (`part z`,`part a`)
) ENGINE=InnoDB;
CREATE TABLE based_catalog_fixture.cycle_a (id BIGINT PRIMARY KEY, peer BIGINT) ENGINE=InnoDB;
CREATE TABLE based_catalog_fixture.cycle_b (id BIGINT PRIMARY KEY, peer BIGINT,
 CONSTRAINT fk_to_a FOREIGN KEY(peer) REFERENCES based_catalog_fixture.cycle_a(id) ON DELETE CASCADE) ENGINE=InnoDB;
ALTER TABLE based_catalog_fixture.cycle_a ADD CONSTRAINT fk_to_b FOREIGN KEY(peer) REFERENCES based_catalog_fixture.cycle_b(id);
CREATE TABLE based_catalog_fixture.keyless (label TEXT, guessed_id INT) ENGINE=InnoDB;
CREATE TABLE based_catalog_fixture.native_details (
 id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
 unsigned_value INT UNSIGNED, enum_value ENUM('a','b'),
 computed INT GENERATED ALWAYS AS (unsigned_value + 1) STORED,
 touched TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
 hidden INT INVISIBLE, label VARCHAR(50),
 INDEX prefix_index(enum_value), INDEX text_prefix(label(3)),
 CONSTRAINT nonnegative CHECK (unsigned_value >= 0)
) ENGINE=InnoDB;
CREATE VIEW based_catalog_fixture.selected_view AS SELECT id FROM based_catalog_fixture.cycle_a;
INSERT INTO based_catalog_fixture.`odd`` table` (`part z`,`part a`) VALUES (7,9);
INSERT INTO based_catalog_fixture.keyless VALUES ('application secret', 123);
CREATE USER based_catalog_reader@'%' IDENTIFIED BY 'fixture_metadata_only';
GRANT REFERENCES, SHOW VIEW ON based_catalog_fixture.* TO based_catalog_reader@'%';

CREATE USER based_catalog_partial@'%' IDENTIFIED BY 'fixture_metadata_only';
GRANT REFERENCES(id) ON based_catalog_fixture.cycle_a TO based_catalog_partial@'%';
