# List a project

List a specific project

**URL** : `/v1/projects/{{urn}}`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns more detailed information on a specific project

```json
{
  "defaultBranch": "main",
  "delegates": [
    {
      "ids": [
        "hyyg555wwkkutaysg6yr67qnu5d5ji54iur3n5uzzszndh8dp7ofue"
      ],
      "type": "indirect",
      "urn": "rad:git:hnrk81wcokr48mkm544kh74kc9fqz84d3rfcy"
    }
  ],
  "description": "ESLint custom rules for Radicle",
  "head": "e6ed6e7b1145ac3f70c999c3c76bca75d9a2e630",
  "name": "eslint-plugin-radicle",
  "urn": "rad:git:hnrkj4yfs7qoew7qbzyayf4s86cyhiezzt6ty"
}
```
