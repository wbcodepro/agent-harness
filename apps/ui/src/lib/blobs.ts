// Object URLs for artifact bytes, fetched once per session view and
// revoked when the view changes, so their memory is given back.

export class BlobUrls {
  private urls = new Map<string, Promise<string>>();
  private made: string[] = [];

  constructor(
    private create: (blob: Blob) => string = (blob) => URL.createObjectURL(blob),
    private revoke: (url: string) => void = (url) => URL.revokeObjectURL(url),
  ) {}

  /** The URL for `sha256`, fetching its bytes with `fetch` the first time. */
  get(sha256: string, fetch: () => Promise<Blob>): Promise<string> {
    let pending = this.urls.get(sha256);
    if (!pending) {
      pending = fetch().then((blob) => {
        const url = this.create(blob);
        this.made.push(url);
        return url;
      });
      this.urls.set(sha256, pending);
      pending.catch(() => this.urls.delete(sha256));
    }
    return pending;
  }

  /** Revokes every URL made so far. */
  clear() {
    for (const url of this.made) this.revoke(url);
    this.made = [];
    this.urls.clear();
  }
}
