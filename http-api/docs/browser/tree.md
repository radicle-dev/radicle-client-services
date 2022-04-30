# Show tree of a folder on a specific commit

Show tree of a folder on a specific commit

**URL** : `/v1/projects/{{urn}}/tree/{{prefix}}`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns a tree of a specific path under a specific commit

```json
{
  "entries": [
    {
      "info": {
        "lastCommit": null,
        "name": "utils.ts",
        "objectType": "BLOB"
      },
      "path": "src/utils.ts"
    },
    {
      "info": {
        "lastCommit": null,
        "name": "web3conv.ts",
        "objectType": "BLOB"
      },
      "path": "src/web3conv.ts"
    }
  ],
  "info": {
    "lastCommit": null,
    "name": "src",
    "objectType": "TREE"
  },
  "path": "src",
  "stats": {
    "branches": 1,
    "commits": 26,
    "contributors": 5
  }
}
```

**Notes**

* Example for a prefix `/{{commit}}/{{folderPath}}`
