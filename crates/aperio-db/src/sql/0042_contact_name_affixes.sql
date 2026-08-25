-- 0042: honorific name prefix + suffix on contacts.
--
-- The two `N` components vCard has always carried and Aperio dropped: the
-- honorific PREFIX ("Prof. Dr.") and SUFFIX ("jun.", "M.D."). Apple, Google
-- (honorificPrefix/honorificSuffix) and Outlook (title/generation) all expose
-- them; nullable TEXT like every other optional contact scalar.
ALTER TABLE contacts ADD COLUMN name_prefix TEXT;
ALTER TABLE contacts ADD COLUMN name_suffix TEXT;

-- ── Re-warm the external contact snapshots ───────────────────────────────
--
-- Same reasoning as 0040 (which did this for jobTitle/department/addresses):
-- cached rows were mapped by adapters that never read the prefix/suffix, so
-- they decode as "the user has none" (`#[serde(default)]`), and a delta sync
-- does not re-send an unchanged contact. On Google and Graph a save sends an
-- absent field as an explicit clear, so the first save of an untouched
-- contact would delete the title it really has on the server. Dropping the
-- sync state too forces a full re-fetch — on Graph the stored deltaLink
-- encodes the OLD `$select` and would keep returning payloads without the
-- new properties forever. The cache is disposable; the cost is one refresh.
DELETE FROM cache_contacts;
DELETE FROM cache_sync_state WHERE scope = 'contacts';
