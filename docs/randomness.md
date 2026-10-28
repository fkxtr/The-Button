# Sources, sampling, and limits

There is no complete index of every public Internet file and no API that samples
that set uniformly. BOTÓN instead samples live metadata from two large public
archives, then validates and downloads one supported candidate.

## Source selection

The initial adapter is chosen with equal probability using Rust's `OsRng`.
Discovery alternates adapters after failure, with at most six attempts and an
overall 90-second deadline. An API outage or a sample containing only unsupported
files can therefore shift the distribution of successful downloads toward the
other source. Production does not use a deterministic seed or a static URL list.

Within a candidate set, the sampler first chooses uniformly among distinct file
extensions, then uniformly among the candidates with that extension. An item with
one PDF and a hundred JPEG derivatives is not automatically a hundred times more
likely to produce a JPEG. Extensions are distinct groups: `.jpg` and `.jpeg` can
both appear as groups even though their underlying format is the same.

## Wikimedia Commons

The adapter calls the MediaWiki action API at
`https://commons.wikimedia.org/w/api.php` with:

- `action=query`, `generator=random`, and `grnnamespace=6` (file pages);
- `grnlimit=12` and `grnfilterredir=nonredirects`;
- `prop=imageinfo` and `iiprop=url|size|mime|sha1`.

It accepts original file URLs only from `upload.wikimedia.org`. Missing file
metadata, unapproved hosts, and unsupported extensions are discarded. The adapter
preserves the SHA-1 supplied by the API for verification by the download engine.

MediaWiki's random-page generator is an index-based sampling service, not a
promise of mathematically uniform file selection. It operates on pages, and its
selection and the adapter's 12-page sample both influence results. Commons' large
photographic collections also make images common.

References:

- [MediaWiki Random API](https://www.mediawiki.org/wiki/API:Random)
- [MediaWiki imageinfo API](https://www.mediawiki.org/wiki/API:Imageinfo)
- [Wikimedia API usage policy](https://foundation.wikimedia.org/wiki/Policy:Wikimedia_Foundation_API_Usage_Guidelines)
- [Commons reuse guidance](https://commons.wikimedia.org/wiki/Commons:Reusing_content_outside_Wikimedia)

## Internet Archive

The adapter uses `https://archive.org/advancedsearch.php` and
`https://archive.org/metadata/{identifier}`:

1. Choose equally among `image`, `texts`, `audio`, and `movies` media types.
2. In three quarters of searches, choose one of 23 decade partitions covering
   1800–2029. In the remaining quarter, omit the year constraint so undated and
   differently dated material remains reachable.
3. Exclude items marked access-restricted and the `malwaremuseum` collection.
4. Request the matching count, sort by identifier, and choose a random single-item
   page among the first `min(count, 10,000)` results.
5. Fetch the selected item's metadata; discard dark, restricted, private,
   unsupported, or malformed entries. Sample among files matching the chosen media
   type with the extension-first strategy above.

Files are requested through `https://archive.org/download/{identifier}/{path}`.
The client follows only validated redirects within the same provider's approved
hosts. Available SHA-1 and size metadata accompany the candidate.

The 10,000-result cap bounds deep paging and creates identifier-order bias.
Decade/media partitions broaden coverage but do not eliminate that bias. Decades
and media groups are weighted equally, not by their population. Empty partitions
lead to another attempt. Large items, rejected signatures, unsupported formats,
and inaccessible files further alter successful-download frequencies. No filter
can prove that every public upload is appropriate or safe.

References:

- [Archive metadata API](https://archive.org/developers/md-read.html)
- [Archive advanced search](https://archive.org/advancedsearch.php)
- [Archive developer documentation](https://archive.org/developers/)
- [Archive terms](https://archive.org/about/terms.php)

## Formats

These extensions are eligible for discovery; subsequent MIME, signature, size,
and checksum validation can still reject a candidate:

| Kind | Extensions |
| --- | --- |
| Images | `jpg`, `jpeg`, `png`, `gif`, `webp`, `tif`, `tiff` |
| Documents | `pdf`, `txt`, `html`, `htm` |
| Audio | `mp3`, `ogg`, `oga`, `flac`, `wav` |
| Video | `mp4`, `webm`, `ogv`, `mov` |

SVG, scripts, native executables, installer packages, arbitrary ZIP archives,
and ISO images are excluded. This release has no official-distribution adapter.
HTML is saved as bytes; BOTÓN never renders it. Text-prefix validation currently
expects UTF-8, so some legitimate historical encodings are rejected.

## Access, licensing, and outages

Both adapters use public unauthenticated metadata endpoints with a descriptive
`BOTON/<version>` user agent and bounded requests. The application does not bypass
authentication, source restrictions, CAPTCHAs, or quotas. Operators can change
availability or API behavior; a failed candidate is not replaced by fake content.

Public access and copyright permission are different questions. Internet Archive
contains material with many different rights statuses. Commons normally hosts
freely licensed or public-domain media, but users must still check the individual
work's license, attribution requirements, and applicable rights before reuse.

In a restricted network, metadata requires `archive.org` and
`commons.wikimedia.org`, and file transfer requires `upload.wikimedia.org` and
Internet Archive's numbered storage hosts. The application itself accepts only
`ia<digits>.archive.org`, `ia<digits>.us.archive.org`, and
`ia<digits>.eu.archive.org` storage names. A network-policy denial cannot be solved
by changing application retry counts or disabling TLS checks.

Fixture tests verify parsers, sampling, and security rules. The separate smoke
example is needed to establish real service reachability and transfer behavior.
