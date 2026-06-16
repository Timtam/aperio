-- Custom "ad-hoc" one-off colors are stored as hidden color labels.
-- The flag marks a label that was composed on the fly (name = hex):
-- it is deduplicated by hex and excluded from the palette-management UI,
-- but resolves exactly like a named label. 0 = normal (named) label.
ALTER TABLE color_labels ADD COLUMN ad_hoc INTEGER NOT NULL DEFAULT 0;
