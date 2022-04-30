# List all projects

List all projects that are hosted on a specific seed

**URL** : `/v1/projects`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns a listing of all projects

```json
[
  {
    "defaultBranch": "master",
    "delegates": [
      {
        "ids": [
          "hydwyyod7zet66r7x5fkckkbqp8zgpgjbnxf5rydaqfmqs3sigguwy"
        ],
        "type": "indirect",
        "urn": "rad:git:hnrkn79arddemsqer5qztr5srznwkgksg5rgo"
      }
    ],
    "description": "A demo project",
    "head": "f11f9cca4e6ad089c80166340b571c5ff94b8ca5",
    "name": "cc-demo",
    "urn": "rad:git:hnrkcnewg4ekq1d18s1qzit4tqshkhqnqnefy"
  },
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
  },
]
```
