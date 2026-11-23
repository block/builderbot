# Upload retries

## Summary

Large uploads fail more often than small ones, and a failed upload is not retried. This note collects what the logs show.

## What happens today

The client sends the file in one request. When the request fails, the user sees an error and has to start again.

## Open questions

- How often do uploads fail?
- Does the failure rate depend on the size of the file?
