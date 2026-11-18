-- What the uploader measured of a sound or video: its loudness, one byte per bar, and its length.
ALTER TABLE attachments ADD COLUMN waveform BLOB;
ALTER TABLE attachments ADD COLUMN duration_ms INTEGER;
