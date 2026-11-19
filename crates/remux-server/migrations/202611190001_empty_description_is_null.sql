-- An empty description is no description: it is read back as a non-empty
-- string now, and as text it overwrote a stored one on every refresh.
UPDATE media SET description = NULL WHERE description = '';
