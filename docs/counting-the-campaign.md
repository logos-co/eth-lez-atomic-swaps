# Counting The Campaign

Maintainer runbook for measuring how the public-testing campaign is going. There is
no `gh` subcommand for Discussions — everything below goes through `gh api graphql`
(or the plain REST `releases` endpoint for the secondary number).

## Primary signal: Swap reports discussions

"How many people told us they tried a swap" is the number we actually want — it is
not gated on something having gone wrong, unlike issues. Swap reports is a plain
(non-form) category, and every report is one discussion in it.

1. Resolve the category's node ID once (its slug is `swap-reports`):

   ```bash
   gh api graphql -f query='
   query($owner:String!, $repo:String!) {
     repository(owner:$owner, name:$repo) {
       discussionCategories(first: 20) { nodes { id slug } }
     }
   }' -f owner=logos-co -f repo=eth-lez-atomic-swaps \
     --jq '.data.repository.discussionCategories.nodes[] | select(.slug=="swap-reports") | .id'
   ```

2. Feed that ID in to count discussions in the category:

   ```bash
   gh api graphql -f query='
   query($owner:String!, $repo:String!, $catid:ID!) {
     repository(owner:$owner, name:$repo) {
       discussions(categoryId: $catid, first: 1) { totalCount }
     }
   }' -f owner=logos-co -f repo=eth-lez-atomic-swaps -f catid="<paste the ID from step 1>"
   ```

Or as a single copy-pasteable pipeline:

```bash
CATID=$(gh api graphql -f query='
query($owner:String!, $repo:String!) {
  repository(owner:$owner, name:$repo) {
    discussionCategories(first: 20) { nodes { id slug } }
  }
}' -f owner=logos-co -f repo=eth-lez-atomic-swaps \
  --jq '.data.repository.discussionCategories.nodes[] | select(.slug=="swap-reports") | .id')

gh api graphql -f query='
query($owner:String!, $repo:String!, $catid:ID!) {
  repository(owner:$owner, name:$repo) {
    discussions(categoryId: $catid, first: 1) { totalCount }
  }
}' -f owner=logos-co -f repo=eth-lez-atomic-swaps -f catid="$CATID" \
  --jq '.data.repository.discussions.totalCount'
```

Swap the `select(.slug=="swap-reports")` filter for `"feedback-help"` to get the
same count for the Feedback & help category (support-question volume, not swap
attempts).

## Secondary denominator: release download counts

```bash
gh api repos/logos-co/eth-lez-atomic-swaps/releases \
  --jq '[.[].assets[]|{name,download_count}]'
```

**Downloads are a ceiling, not a count.** A download only proves an asset was
fetched — it does not prove anyone ran the app, and it overcounts: CI/CD mirrors,
package-manager bots, and CDN/proxy re-fetches all inflate the number. Treat it as
"at most this many people could have tried it," and use the Swap reports
discussion count above as the actual (undercounted, but honest) signal of how many
people tried and told us.
