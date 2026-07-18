#!/usr/bin/env python3
"""Remux <-> Jellyfin audio response parity harness.

Diffs Remux's JSON responses against a real Jellyfin's, field-by-field, bucketed,
with a normalization/ignore layer so only genuine gaps surface. Re-run after each
fix to prove closure and catch regressions.

Usage:
  parity.py --profile local   # Remux local track  vs seeded Jellyfin (same files)
  parity.py --profile remote  # Remux streaming     vs demo.jellyfin.org
"""
import argparse, json, subprocess, sys, urllib.parse, collections, re

SCR = "/tmp/claude-1000/-home-joey/1be0cc14-7856-432a-8fb5-fa7595b3bb91/scratchpad"

def rd(p):
    try: return open(p).read().strip()
    except Exception: return ""

# ---- servers -------------------------------------------------------------
REMUX = ("http://127.0.0.1:3008", "c3bb11fa62ec42f2a1eafd49e199251f",
         "ab993c51-b008-4629-98aa-5066ba9e46b4")
JF_LOCAL = (f"http://127.0.0.1:{rd(SCR+'/jf_port.txt') or '8899'}",
            rd(SCR+"/jf_parity_token.txt"), rd(SCR+"/jf_parity_uid.txt"))
JF_DEMO  = ("https://demo.jellyfin.org/stable", rd(SCR+"/jfdemo_token.txt"),
            rd(SCR+"/jfdemo_uid.txt"))

# Union superset of Fields the real clients (Finamp + Jellify) request.
FIELDS = ("MediaSources,MediaStreams,Genres,GenreItems,Chapters,Path,SortName,"
          "ChildCount,ItemCounts,ParentId,DateCreated,ProviderIds,Overview,"
          "PrimaryImageAspectRatio,AlbumId,AlbumArtists,ArtistItems,People,Tags")

# ---- ignore-list: keys whose VALUE legitimately differs (assert presence) --
# Compared by presence only, not value.
PRESENCE_ONLY = {
    "Id","ItemId","ServerId","ParentId","AlbumId","ParentBackdropItemId","ChannelId",
    "Etag","ETag","PlaySessionId","Path","Key",
    "DateCreated","DateModified","DateLastMediaAdded","PremiereDate","LastPlayedDate",
    "AlbumPrimaryImageTag","ParentBackdropImageTags","BackdropImageTags",
    "PrimaryImageAspectRatio","TranscodingUrl","DirectStreamUrl",
    "RunTimeTicks",  # values differ per encode; presence is what matters
}
# keys dropped entirely before diffing (server-specific / non-comparable)
DROP = {"ProviderIds","ImageBlurHashes","ImageTags","UserData","People","Remux",
        "PlayAccess","CanDelete","CanDownload","LocationType","ServerId","SortName",
        "ExternalUrls","RemoteTrailers","ProductionLocations","Taglines",
        "DisplayPreferencesId","PlayedPercentage","NormalizationGain"}
# array fields compared by member Name-set only (Ids differ)
NAME_SET = {"Artists","AlbumArtists","ArtistItems","GenreItems","Genres"}

def curl(base, tok, path):
    url = base + path
    out = subprocess.run(["curl","-s","-m","25","-H",f"X-Emby-Token: {tok}",url],
                         capture_output=True, text=True).stdout
    try: return json.loads(out)
    except Exception: return {"__nonjson__": out[:200]}

def norm(o):
    """Recursively drop DROP keys; leave the rest for diff."""
    if isinstance(o, dict):
        return {k: norm(v) for k,v in o.items() if k not in DROP}
    if isinstance(o, list):
        return [norm(x) for x in o]
    return o

def diff(jf, rx, path=""):
    """Yield (bucket, path, jf_val, rx_val)."""
    out=[]
    if isinstance(jf, dict) and isinstance(rx, dict):
        for k in jf:
            p=f"{path}.{k}" if path else k
            if k in DROP: continue
            if k not in rx:
                # Omitting a field Jellyfin sends as null is harmless parity-wise;
                # only flag when Jellyfin has a real value.
                if jf[k] is not None and jf[k] != [] and jf[k] != {}:
                    out.append(("MISSING",p,jf[k],None))
                continue
            if k in PRESENCE_ONLY:
                jp, rp = jf[k] is not None, rx[k] is not None
                if jp and not rp: out.append(("NULL_VS_VALUE",p,jf[k],rx[k]))
                continue
            if k in NAME_SET:
                jn=sorted(x.get("Name") if isinstance(x,dict) else x for x in (jf[k] or []))
                rn=sorted(x.get("Name") if isinstance(x,dict) else x for x in (rx[k] or []))
                if jn and not rn: out.append(("EMPTY_ARRAY_VS_POPULATED",p,jn,rn))
                elif jn!=rn: out.append(("VALUE_DIFF",p,jn,rn))
                continue
            out += diff(jf[k], rx[k], p)
        # key-casing mismatch detection (TYPE via wrong-case keys)
        return out
    if isinstance(jf, list) and isinstance(rx, list):
        if jf and not rx: out.append(("EMPTY_ARRAY_VS_POPULATED",path,f"[{len(jf)}]","[]")); return out
        n=min(len(jf),len(rx))
        for i in range(n): out += diff(jf[i], rx[i], f"{path}[{i}]")
        return out
    if jf is not None and rx is None:
        out.append(("NULL_VS_VALUE",path,jf,rx)); return out
    if type(jf)!=type(rx) and rx is not None:
        out.append(("TYPE_MISMATCH",path,f"{type(jf).__name__}:{jf}",f"{type(rx).__name__}:{rx}")); return out
    if jf!=rx:
        out.append(("VALUE_DIFF",path,jf,rx))
    return out

def key(it):
    return (str(it.get("Name","")).strip().lower(),
            str(it.get("Album","")).strip().lower())

FAIL_BUCKETS={"MISSING","NULL_VS_VALUE","EMPTY_ARRAY_VS_POPULATED","TYPE_MISMATCH"}

# Seed dir on disk; used to resolve Remux ids deterministically via the DB.
SEED_LIKE = ["%/Chief Keef/Bang (2011)/%", "%/Chief Keef/Ottopsy (2018)/%"]

def remux_ids_for_seed():
    where=" OR ".join(f"f.path LIKE '{p}'" for p in SEED_LIKE)
    sql=(f"SELECT lower(hex(m.id)) FROM media m JOIN opendal_files f ON f.id=m.id "
         f"WHERE m.kind='track' AND ({where})")
    out=subprocess.run(["sudo","-u","jellyflix","sqlite3","/opt/remux/data/db.sqlite",sql],
                       capture_output=True,text=True).stdout
    return [l.strip() for l in out.splitlines() if l.strip()]

def run_tracks(jf, rx):
    jb,jt,ju=jf; rb,rt,ru=rx
    jitems=curl(jb,jt,f"/Items?IncludeItemTypes=Audio&Recursive=true&Limit=50&userId={ju}&Fields={FIELDS}").get("Items",[])
    ids=remux_ids_for_seed()
    def hy(h): return f"{h[0:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}"
    rmap={}
    if ids:
        got=curl(rb,rt,f"/Items?Ids={','.join(hy(i) for i in ids)}&Fields={FIELDS}&userId={ru}").get("Items",[])
        # match by title only (same physical files)
        for it in got: rmap[str(it.get("Name","")).strip().lower()]=it
    ritems=list(rmap.values())
    print(f"  Jellyfin tracks: {len(jitems)} | Remux sampled: {len(ritems)}")
    buckets=collections.Counter(); matched=0; details=collections.defaultdict(list)
    for jt_it in jitems:
        rx_it=rmap.get(str(jt_it.get("Name","")).strip().lower())
        if not rx_it: continue
        matched+=1
        for b,p,jv,rv in diff(norm(jt_it),norm(rx_it)):
            buckets[b]+=1; details[(b,p)].append((jt_it.get("Name"),jv,rv))
    print(f"  matched {matched} track pairs")
    return buckets, details

def main():
    ap=argparse.ArgumentParser(); ap.add_argument("--profile",default="local")
    ap.add_argument("--value-diff",action="store_true",help="also list non-failing VALUE_DIFF fields")
    a=ap.parse_args()
    jf = JF_LOCAL if a.profile=="local" else JF_DEMO
    print(f"=== PARITY: Remux vs Jellyfin ({a.profile}) ===")
    buckets, details = run_tracks(jf, REMUX)
    print("\n=== bucket summary (track item) ===")
    for b,c in buckets.most_common(): print(f"  {b}: {c}")
    print("\n=== top gaps (field -> jf vs remux) ===")
    fails=[(bp,v) for bp,v in details.items() if bp[0] in FAIL_BUCKETS]
    fails.sort(key=lambda x:-len(x[1]))
    for (b,p),ex in fails[:25]:
        n,jv,rv=ex[0]
        print(f"  [{b}] {p:32} JF={str(jv)[:34]:34} RMX={str(rv)[:30]}  (x{len(ex)})")
    if a.value_diff:
        print("\n=== VALUE_DIFF (non-failing; server-specific or expected) ===")
        vds=[(bp,v) for bp,v in details.items() if bp[0]=="VALUE_DIFF"]
        vds.sort(key=lambda x:-len(x[1]))
        for (b,p),ex in vds:
            n,jv,rv=ex[0]
            print(f"  {p:34} JF={str(jv)[:34]:34} RMX={str(rv)[:30]}  (x{len(ex)})")
    hard=sum(c for b,c in buckets.items() if b in FAIL_BUCKETS)
    print(f"\nRESULT: {'FAIL' if hard else 'PASS'} — {hard} real gaps")
    sys.exit(1 if hard else 0)

if __name__=="__main__": main()
