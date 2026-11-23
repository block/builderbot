# Upload retries: findings and plan

## Summary

Uploads over 50 MB fail about one time in twelve, and a failed upload is retried only by hand. The client gives up on the first error, so the user has to start the whole file again.

This note covers what the logs show, where the failures come from, and a plan for retrying them.

## What happens today

The client sends the file to the upload API in one request. The API streams it to the object store and answers when the last byte is written.

```pikchr
box "Client" fit
arrow
box "Upload API" fit
arrow
box "Retry queue" fit
arrow
box "Object store" fit
```

When the request fails part-way, nothing is kept: the bytes already received are dropped and the client shows an error.

## Findings

- Failures rise with file size: 1% under 10 MB, 8% over 50 MB.
- Three in four failures are connection resets on mobile networks.
- The API has no way to resume a partial upload.
- Retrying the whole file succeeds on the second attempt nine times in ten.

| Client  | Failed uploads | Retried by hand |
| ------- | -------------- | --------------- |
| Web     | 3.1%           | 62%             |
| iOS     | 7.4%           | 38%             |
| Android | 8.9%           | 35%             |

## Plan

Split each upload into 8 MB parts and retry a failed part on its own. The API already accepts ranged writes, so only the client changes.

1. Send the parts in order, three at a time.
2. Retry a failed part up to four times, waiting longer each time.
3. Give up on the upload when one part fails five times.

```ts
const PART_SIZE = 8 * 1024 * 1024;
const MAX_ATTEMPTS = 5;
```

## Rollout

The change ships behind the `chunked-uploads` flag, to one client at a time.

1. Web, for staff accounts.
2. Web, for everyone.
3. iOS and Android, once the web failure rate is under 1%.

## Open questions

- Should a paused upload survive an app restart?
- What does the object store charge for parts that are never finished?
