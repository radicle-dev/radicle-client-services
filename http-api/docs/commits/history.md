# Show commit history

Show the commit history off a specific project

**URL** : `/v1/projects/{{urn}}/commits`

**Method** : `GET`

**Params** :

| Parameter    | Type    | Required?  | Description                                                          |
| -------------|---------|------------|----------------------------------------------------------------------|
| `parent`     | string  |            | Commit hash from where the revwalk should begin                      |
| `since`      | string  |            | Date ISO string since when commits should be listed                  |
| `until`      | string  |            | Date ISO string until when commits should be included in the listing |
| `page`       | string  |            | Which page should be shown                                           |
| `per_page`   | string  |            | How many commits per page should be fetched                          |
| `verified`   | boolean |            | If the commit signature should be verified against the peer keys for all queried commits |

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns a listing off all commits on a project

```json
{
  "headers": [
    {
      "context": {
        "committer": null
      },
      "header": {
        "author": {
          "email": "sebastinez@me.com",
          "name": "Sebastian Martinez"
        },
        "committer": {
          "email": "sebastinez@me.com",
          "name": "Sebastian Martinez"
        },
        "committerTime": 1644487228,
        "description": "",
        "sha1": "e6ed6e7b1145ac3f70c999c3c76bca75d9a2e630",
        "summary": "Change rule name"
      }
    },
    {
      "context": {
        "committer": null
      },
      "header": {
        "author": {
          "email": "sebastinez@me.com",
          "name": "Sebastian Martinez"
        },
        "committer": {
          "email": "sebastinez@me.com",
          "name": "Sebastian Martinez"
        },
        "committerTime": 1644424659,
        "description": "",
        "sha1": "452cfe5255036287dc455e0b0fd75b8e767dcbca",
        "summary": "Initial commit"
      }
    }
  ],
  "stats": {
    "branches": 1,
    "commits": 2,
    "contributors": 1
  }
}
```
