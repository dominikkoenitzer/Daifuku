# Download page

The page behind https://get-daifuku.vercel.app: plain static files, no build step.
It names no author and does not link to this repository.

- `index.html` is the page.
- `vercel.json` holds the download addresses, each a temporary redirect:
  - `/download/windows`: the newest release's `daifuku.exe`
  - `/download/windows-arm64`: the newest release's `daifuku-arm64.exe`
  - `/schema.json`: `schema.json` on the main branch
- `llms.txt` describes Daifuku for AI assistants.
- `skill/daifuku/` is the assistant skill, and `skill/daifuku.zip` is that folder zipped. Rebuild the zip after an edit, from this folder:

  ```
  python -c "import shutil; shutil.make_archive('skill/daifuku', 'zip', 'skill', 'daifuku')"
  ```

Vercel deploys this folder whenever a push to main changes it. `.vercelignore` keeps this README off the site.
