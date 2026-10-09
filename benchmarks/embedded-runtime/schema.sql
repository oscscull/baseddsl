-- Verbatim table section of `based gen sql`; regeneration checked by the runner.
CREATE TABLE `owner` (
  `id` INTEGER PRIMARY KEY AUTOINCREMENT,
  `name` TEXT NOT NULL
);
CREATE TABLE `item` (
  `id` INTEGER PRIMARY KEY AUTOINCREMENT,
  `owner_id` INTEGER NOT NULL,
  `name` TEXT NOT NULL,
  `value` INTEGER NOT NULL
);
CREATE INDEX `idx_item_owner` ON `item` (`owner_id`);
