# Cloudflare R2 setup and CDN guide

Connect Mazit to R2 for uploads, then serve podcast feeds and audio through a custom domain on Cloudflare’s Free plan. This guide uses ordinary CDN caching with no paid delivery add-ons or Worker.

Podcast players poll `rss.xml` for episodes and fetch audio from its enclosure URLs.

Checked against Mazit’s source and Cloudflare documentation on September 23, 2026.

## Budget: pay for storage, use free delivery features

Use this configuration:

- **Cloudflare Free plan:** ordinary CDN caching and the two Cache Rules below. The Free plan includes ten Cache Rules. See [plan pricing](https://www.cloudflare.com/plans/) and [Cache Rules availability](https://developers.cloudflare.com/cache/how-to/cache-rules/#availability).
- **An existing domain:** use a subdomain you already control. Purchasing or renewing a domain is a separate expense; this recipe assumes one is already available.
- **R2 Standard storage:** keep request usage inside the included allowance. Avoid Infrequent Access for this setup because it adds retrieval charges and does not receive the Standard free tier.
- **No paid upgrades:** skip Argo, paid Smart Shield packages, Cache Reserve, Workers Paid, and paid monitoring. Smart Tiered Cache is not part of this recipe.

R2 Standard currently includes these monthly allowances:

| Item                                  | Included usage |
| ------------------------------------- | -------------- |
| Storage                               | 10 GB-month    |
| Class A operations, including uploads | 1 million      |
| Class B operations, including reads   | 10 million     |
| Internet egress                       | Free           |

Above the allowances, storage is $0.015/GB-month, Class A operations are $4.50/million, and Class B operations are $0.36/million, subject to billing-unit rounding. **A storage-only bill requires keeping operations within their free allowances.** CDN misses, uncached feed reads, and multipart uploads still consume operations. See [R2 pricing](https://developers.cloudflare.com/r2/pricing/).

Check **Manage Account → Billing → Billable Usage** and R2 operation totals. A [budget alert](https://developers.cloudflare.com/billing/manage/budget-alerts/) can notify you, but does not cap charges or stop traffic. Mazit has no automatic request-budget cutoff, so this public-bucket setup cannot guarantee a storage-only bill under arbitrary traffic. If the request budget is a hard limit, serving and uploads must stop before it is exceeded; alerts alone do not enforce that limit.

## The two connections

| Connection                       | Address                                         | Purpose                                                  |
| -------------------------------- | ----------------------------------------------- | -------------------------------------------------------- |
| Mazit → R2                       | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` | Authenticated uploads and deletes using S3 credentials.  |
| Podcast player → Cloudflare → R2 | `https://audio.example.com`                     | Public RSS and audio, with CDN caching on the read path. |

Use **`auto` as the S3 region**. `EEUR` is a physical location, not a signing region. R2 also accepts `us-east-1` as an alias. See [R2’s region reference](https://developers.cloudflare.com/r2/api/s3/api/#bucket-region).

Use a **custom domain for production**. The public `r2.dev` URL is rate-limited and does not support CDN caching. Uploads continue to use the S3 endpoint after you attach a domain. See [R2 and Cloudflare Cache](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/).

> Choose the final hostname and folder prefix before adding subscriptions. The current Mazit build prevents changing the storage destination of a library that already contains subscriptions, including its public base URL. Moving existing feeds requires migration or a separate library.

All domains and IDs below are placeholders. The example bucket is `podcasts`, with the optional folder prefix `mazit`.

## 1. Connect a public domain

Use a dedicated bucket for files intended to be public. A folder prefix organizes objects; it does not restrict access to the rest of the bucket.

1. Have an existing domain in the same Cloudflare account as the R2 bucket, on the Free plan.
2. Open **R2 Object Storage → podcasts → Settings**.
3. Under **Custom Domains**, select **Add** or **Connect Domain**.
4. Enter a hostname such as `audio.example.com`, continue, review the DNS record, and connect.
5. Wait for the domain’s status to become **Active**.

Use this bucket workflow instead of manually creating a CNAME to `r2.dev`, which is unsupported. A request to the hostname’s root does not list bucket contents; test a known object path once one exists.

Public podcast URLs must serve files directly, without a login or interactive challenge. After all readers use the custom domain, disable the development URL if it is no longer needed.

Reference: [Public buckets and custom domains](https://developers.cloudflare.com/r2/buckets/public-buckets/).

## 2. Create S3 credentials

1. Open the **R2 overview → Account Details → API Tokens → Manage**.
2. Create a **User API token** for your desktop, or an **Account API token** if your administrator wants account-owned credentials.
3. Give it a recognizable name, such as `Mazit desktop`.
4. Select **Object Read & Write**, scoped to the `podcasts` bucket.
5. Create the token and securely save its **Access Key ID** and **Secret Access Key**. The secret is shown only at creation.

Mazit needs write and delete access for publishing, multipart uploads, verification, and episode cleanup. Read-only access is insufficient; bucket administration permissions are unnecessary.

Enter the generated S3 key pair in `~/.config/mazit/config.toml`. Do not substitute your account ID, a general Cloudflare bearer token, or Global API Key. Never put credentials in RSS URLs or commit them to documentation.

Reference: [R2 authentication](https://developers.cloudflare.com/r2/api/tokens/).

## 3. Fill in Mazit’s TOML config

Open **config.toml** in the desktop app and fill in its `[s3]` table. The complete schema and example are in the [README](../README.md#storage-configuration).

| TOML key             | Value                                           | Notes                                                                                                                     |
| -------------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `endpoint`           | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` | Copy the S3 API hostname from the bucket settings. Remove the trailing `/podcasts`; Mazit supplies the bucket separately. |
| `region`             | `auto`                                          | Do not use `EEUR`.                                                                                                        |
| `bucket`             | `podcasts`                                      | Bucket name only.                                                                                                         |
| `root`               | `mazit`                                         | Optional; blank is also valid. Choose before creating subscriptions.                                                      |
| `public_base_url`    | `https://audio.example.com`                     | The public bucket origin, without the bucket name or folder prefix.                                                       |
| `access_key_id`      | Your generated Access Key ID                    | From the R2 token creation screen.                                                                                        |
| `secret_access_key`  | Your generated Secret Access Key                | From the same screen.                                                                                                     |

For jurisdiction-restricted buckets, retain the jurisdiction-specific S3 hostname shown by Cloudflare, such as `.eu.r2.cloudflarestorage.com`. A location hint such as EEUR does not itself imply that endpoint. See [R2 endpoint requirements](https://developers.cloudflare.com/r2/api/tokens/).

Mazit builds public URLs like this:

```text
Public base:  https://audio.example.com
Prefix:       mazit
Source:       playlist-PLAYLIST_ID

Feed:  https://audio.example.com/mazit/playlist-PLAYLIST_ID/rss.xml
Audio: https://audio.example.com/mazit/playlist-PLAYLIST_ID/VIDEO_ID.m4a
```

The source folder is illustrative. Copy the real feed URL from Mazit. Putting `mazit` in both the public base URL and prefix would duplicate that folder.

Save the file and select **Reload config**. Mazit uploads a uniquely named text file, reads its contents through the public URL, checks them, and deletes the object. It then refreshes every saved YouTube subscription and publishes changes to S3. The verification checks write access, public reads, and cleanup; it does not verify CDN hits or byte ranges.

Once connected, add a playlist or channel, let synchronization finish, and select **Copy RSS URL** to subscribe in your podcast app.

## 4. Configure CDN caching

### Start with audio caching and fresh feeds

The current uploader sets:

| Object                     | Content-Type                         | Cache-Control          |
| -------------------------- | ------------------------------------ | ---------------------- |
| Audio `.m4a`               | `audio/mp4`                          | `public, max-age=3600` |
| Feed `rss.xml`             | `application/rss+xml; charset=utf-8` | `no-cache`             |
| Temporary connection probe | `text/plain`                         | `no-cache`             |

`no-cache` allows storage but requires validation before reuse; it is different from `no-store`. The following recipe explicitly bypasses the edge cache for feeds and probes. See [Origin Cache Control](https://developers.cloudflare.com/cache/concepts/cache-control/).

Open the parent domain’s zone in Cloudflare, then **Caching → Cache Rules → Create rule**. Replace the hostname and prefix in both expressions.

**Rule A: cache Mazit audio**

```text
(http.host eq "audio.example.com" and starts_with(http.request.uri.path, "/mazit/") and ends_with(http.request.uri.path, ".m4a"))
```

- **Cache eligibility:** Eligible for cache.
- **Edge TTL:** Use the origin cache-control header; bypass cache if it is absent.
- **Respect Strong ETags:** On.

This starts with Mazit’s one-hour audio TTL. Preserve R2’s response cache headers for podcast players.

**Rule B: bypass feeds and probes**

```text
(http.host eq "audio.example.com" and starts_with(http.request.uri.path, "/mazit/") and not ends_with(http.request.uri.path, ".m4a"))
```

- **Cache eligibility:** Bypass cache.

If your prefix is blank, remove the `starts_with(...)` condition from both rules. Place these rules after broader rules and check that later rules do not override them. Do not apply a long TTL to the entire bucket.

References: [Cache Rule settings](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/), [rule order](https://developers.cloudflare.com/cache/how-to/cache-rules/order/), and [ETags](https://developers.cloudflare.com/cache/reference/etag-headers/).

### Skip Smart Tiered Cache for this setup

Ordinary edge caching, byte-range playback, and ETag validation work without Smart Tiered Cache. Leave it out of the setup; cache misses go to R2 directly.

Cloudflare’s [Smart Shield setup](https://developers.cloudflare.com/smart-shield/get-started/) describes both a free entry path and purchasable packages. This guide does not assume that a package or trial is free indefinitely. Do not activate a paid package to follow it. If your account already includes Smart Tiered Cache at no additional cost, it is optional; the instructions do not depend on it.

### Optional: keep audio at the edge for seven days

After validating the baseline, change **Rule A → Edge TTL** to ignore origin cache-control and use **7 days**. Apply this override only at Cloudflare’s edge, preserving R2’s response cache headers for podcast players. Set error-status TTLs (`400–599`) to **Do not cache** so missing objects do not remain missing for days.

This uses the existing Cache Rule without buying an add-on. Longer caching can reduce repeat R2 reads; use it once you are prepared to purge changed or removed audio. See [Edge TTL overrides](https://developers.cloudflare.com/cache/how-to/cache-rules/examples/edge-ttl/) and [status-code TTLs](https://developers.cloudflare.com/cache/how-to/cache-rules/examples/cache-ttl-by-status-code/).

**Longer TTLs need purging.** Mazit names audio by video ID, not content hash. It deletes removed episodes from R2 but does not purge Cloudflare. After replacing or deleting audio, use **Caching → Configuration → Custom Purge** to purge the exact public URL. Until expiry or purge, a cached copy may remain available. Purging the CDN cannot remove copies already downloaded by players. See [single-file purging](https://developers.cloudflare.com/cache/how-to/purge-cache/purge-by-single-file/).

Keep enclosure URLs stable. Query-string cache busters fragment the cache. Only ignore query strings deliberately, after confirming they never change content or access behavior.

## 5. Byte ranges and ETags

Byte-range requests let players seek or download only part of an episode. A valid request such as `Range: bytes=0-1023` should return `206 Partial Content`, a matching `Content-Range`, and the requested bytes. Mazit also prepares fast-start M4A files, placing playback metadata near the beginning.

An ETag identifies a representation. A client sends it in `If-None-Match`; an unchanged object can return `304 Not Modified` without the body. Treat ETags as opaque values, especially for multipart uploads. With `If-Range`, a matching validator preserves range behavior; a mismatch returns the full representation.

Preserve media bytes and validators. Avoid compression and body transformations for M4A delivery. References: [range behavior](https://developers.cloudflare.com/cache/reference/range-requests/) and [ETag handling](https://developers.cloudflare.com/cache/reference/etag-headers/).

### Keep origin range fetching at its default

Ordinary range playback works from complete cached objects. Leave the optional **Origin Range Requests** setting unconfigured for this recipe.

Opting in can fetch files in pieces, but may turn one playback request into several R2 reads. Because R2 egress is already free, fewer transferred bytes do not necessarily mean a smaller bill. It is not a guaranteed cost optimization. See [Origin Range Requests settings](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/#origin-range-requests).

## 6. Verify the result

Use an already uploaded episode on the custom domain. Replace `PLAYLIST_ID` and `VIDEO_ID` with the actual source folder and filename. These commands work in fish, zsh, and bash and require no credentials.

### Check cache hits

Run this twice, sequentially, from the same machine:

```sh
curl -sS -D - -o /dev/null --max-time 120 \
  'https://audio.example.com/mazit/playlist-PLAYLIST_ID/VIDEO_ID.m4a'
```

This downloads the full episode to `/dev/null`; start with a small file and let each request complete. A full GET provides a clearer cache-fill check than HEAD alone.

| Response               | Meaning                                                                                           |
| ---------------------- | ------------------------------------------------------------------------------------------------- |
| `CF-Cache-Status: HIT` | Served from cache. `Age` usually reports its age in seconds.                                      |
| `MISS`                 | The request missed; an eligible response may fill the cache.                                      |
| `REVALIDATED`          | A cached object was checked and found unchanged.                                                  |
| `DYNAMIC` or `BYPASS`  | Investigate rules and headers for audio. Uncached behavior is intentional for RSS in this recipe. |

A second-request HIT is useful evidence, not a guarantee: routing, fills, and eviction affect the result. See [cache response meanings](https://developers.cloudflare.com/cache/concepts/cache-responses/).

### Check a real range

```sh
curl -sS -D - -o /dev/null --max-time 30 \
  -H 'Accept-Encoding: identity' -H 'Range: bytes=0-1023' \
  -w '\nReceived bytes: %{size_download}\n' \
  'https://audio.example.com/mazit/playlist-PLAYLIST_ID/VIDEO_ID.m4a'
```

For an episode larger than 1 KiB, expect `206`, `Content-Range: bytes 0-1023/TOTAL`, `Content-Length: 1024`, and `Received bytes: 1024`. Repeat and inspect cache status too. `Accept-Ranges: bytes` alone does not prove seeking works.

### Check ETag validation

Copy the exact ETag from the preceding response, preserving quotes and any `W/` prefix:

```sh
curl -sS -D - -o /dev/null --max-time 30 \
  -H 'Accept-Encoding: identity' \
  -H 'If-None-Match: "PASTE-EXACT-ETAG-HERE"' \
  'https://audio.example.com/mazit/playlist-PLAYLIST_ID/VIDEO_ID.m4a'
```

An unchanged object should return `304` with no body. This validates conditional requests; inspect cache status separately to establish CDN use.

### Check RSS freshness and playback

```sh
curl -sS -D - --max-time 30 \
  'https://audio.example.com/mazit/playlist-PLAYLIST_ID/rss.xml'
```

Expect successful RSS output and enclosure URLs on your custom hostname. After an actual feed change and a successful Mazit refresh, repeat and confirm the new contents appear. Finally, subscribe and seek in a real podcast app.

## Limits and further optimization

- **Cacheable file size:** the Free plan limit is 512 MB per file. Larger R2 objects may be served but do not get that cache benefit. Work within this limit rather than upgrading plans. See [R2 cache limits](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/).
- **Ranges do not bypass that limit:** the complete object size still counts. See [range caching limitations](https://developers.cloudflare.com/cache/reference/range-requests/#caching-limitations).
- **Cache Reserve does not add a layer here:** direct R2 public-bucket requests through a zone’s domain do not use it. See [Cache Reserve limitations](https://developers.cloudflare.com/cache/advanced-configuration/cache-reserve/#limits).
- **TTL is not guaranteed residency:** unpopular objects can be evicted before their freshness period ends. See [retention versus freshness](https://developers.cloudflare.com/cache/concepts/retention-vs-freshness/).
- **Avoid unnecessary origin traffic:** run verification checks when configuring or diagnosing a problem, rather than repeatedly warming the entire library. Keep stable URLs and use selective purges so useful cached audio stays available.

For more aggressive caching, future Mazit changes could publish content-versioned audio filenames with `Cache-Control: public, max-age=31536000, immutable`. Changed bytes would get a new URL. Automatic purging on removal would support timely withdrawal. **The current app does not implement these features.**

Shared RSS caching could use a short origin-provided TTL such as `public, max-age=0, s-maxage=60`, together with feed cache eligibility that respects the origin. That needs an uploader change and deliberately trades up to a minute of freshness for fewer reads. Dashboard TTL overrides have plan-dependent minimums. See the [cache TTL reference](https://developers.cloudflare.com/cache/how-to/edge-browser-cache-ttl/).

## Troubleshooting

| Problem                                        | Check                                                                                                                                                                                                           |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Signature error or AccessDenied                | Region `auto`, correct S3 hostname, no bucket suffix in endpoint, generated key pair, bucket scope, expiry/IP restrictions, and accurate system clock.                                                          |
| Upload works but connection verification fails | Active public domain, correct bucket and prefix, raw file access without login/challenges, and permission to delete the probe. Bucket locks can prevent cleanup.                                                |
| Domain root returns an error                   | Test an actual object URL; public R2 does not list the bucket at its root.                                                                                                                                      |
| Audio never hits cache                         | Custom hostname, matching rules, rule precedence, headers, object size, Development Mode, and identical URLs without cache busters.                                                                             |
| Range request returns full `200`               | Valid range, nonempty object, no mismatched `If-Range`, and no encoding/body transformations.                                                                                                                   |
| Feed or removed audio is stale                 | Correct bypass rule and successful publishing; purge previously cached URLs. Players maintain their own refresh schedules and downloaded copies.                                                                |
| Changing the public URL is rejected            | An existing library is bound to its destination. To start a new library, quit Mazit, archive `~/Library/Application Support/Mazit`, then relaunch. This does not migrate subscribers or old files. |

## Launch checklist

- [ ] The domain uses the Free plan; no paid delivery packages or trials were added.
- [ ] The bucket uses Standard storage, and R2 request usage is within the free allowances.
- [ ] Custom domain is Active on the intended public bucket.
- [ ] Bucket-scoped Object Read & Write credentials are saved in `~/.config/mazit/config.toml`.
- [ ] Region, endpoint, prefix, and public URL are correct.
- [ ] Reload config succeeds before adding subscriptions.
- [ ] Audio caches; RSS and connection probes bypass the edge cache.
- [ ] A real audio request shows a cache HIT.
- [ ] A range request returns `206` and the expected bytes.
- [ ] An unchanged object returns `304` for its exact ETag.
- [ ] RSS updates, playback and seeking work, and the purge policy is understood.

### Implementation references

Current behavior comes from [config.rs](../src/config.rs) (TOML config), [storage.rs](../src/storage.rs) (S3, headers, public URLs, verification), [engine.rs](../src/engine.rs) (publication, naming, deletion), [database.rs](../src/database.rs) (destination binding), and [rss.rs](../src/rss.rs) (RSS and enclosure URLs).
