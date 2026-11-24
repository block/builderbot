# Upload retries: findings and plan

## Summary

Uploads over 50 MB fail about one time in twelve, and a failed upload is never retried. The client gives up on the first error, so the user has to start the whole file again.

## What happens today

The client sends the file to the upload API in one request. The API streams it to the object store and answers when the last byte is written.

When the request fails part-way, nothing is kept: the bytes already received are dropped and the client shows an error.

We suspect the load balancer's idle timeout, which is 60 seconds.

## Findings

- Failures rise with file size: 1% under 10 MB, 8% over 50 MB.
- Most failures are connection resets on mobile networks.

| Client | Failed uploads | Retried by hand |
| ------ | -------------- | --------------- |
| Web    | 3.1%           | 62%             |
| iOS    | 7.4%           | 41%             |

## Open questions

- Can the API resume a partial upload?
- Should a paused upload survive an app restart?
