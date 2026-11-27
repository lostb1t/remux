-- An addon's content types become optional: JSON `null` means every type the
-- addon supports, `[]` means none. Until now `[]` meant every type, so a user
-- could not untick an addon's last type; existing `[]` keep their meaning.
UPDATE addons SET types = 'null' WHERE types = '[]';
