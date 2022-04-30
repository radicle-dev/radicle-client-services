# API documentation

This document describes the endpoints for the radicle-client-services/http-api.

#### Contents

- [API documentation](#api-documentation)
      - [Contents](#contents)
  - [1. Overview](#1-overview)
  - [2. Authentication](#2-authentication)
  - [3. Resources](#3-resources)
    - [3.1 General](#31-general)
    - [3.2 Projects](#32-projects)
      - [3.3 Commits](#33-commits)
      - [3.4 Remotes](#34-remotes)
      - [3.5 Browser](#35-browser)
      - [3.6 Patches](#36-patches)
    - [3.7 Sessions](#37-sessions)

## 1. Overview

Radicle HTTP API is a JSON-based API.

All requests must be secure, i.e. `https`, not `http`.

## 2. Authentication

To be written..

## 3. Resources

The API is RESTful and arranged around resources. All requests must be made using `https`.

### 3.1 General

* [Info](info/get.md) : `GET /`
* [Peer](peer/get.md) : `GET /v1/peer`

### 3.2 Projects

* [List all projects](projects/list.md) : `GET /v1/projects`
* [List a project](projects/get.md) : `GET /v1/projects/{{urn}}`

#### 3.3 Commits

* [Show history of all commits](commits/history.md) : `GET /v1/projects/{{urn}}/commits`
* [Show a specific commit](commits/commit.md) : `GET /v1/projects/{{urn}}/commits/{{sha}}`

#### 3.4 Remotes

* [Show all remotes](commits/history.md) : `GET /v1/projects/{{urn}}/remotes`
* [Show heads of a specific remote](commits/history.md) : `GET /v1/projects/{{urn}}/remotes/{{peer}}`

#### 3.5 Browser

* [Show tree](browser/tree.md) : `GET /v1/projects/{{urn}}/tree/{{prefix}}`
* [Show blob](browser/blob.md) : `GET /v1/projects/{{urn}}/blob/{{sha}}/{{path}}`
* [Show readme](browser/readme.md) : `GET /v1/projects/{{urn}}/readme/{{sha}}`

#### 3.6 Patches

* [Show patches](patches/list.md) : `GET /v1/projects/{{urn}}/patches`

### 3.7 Sessions

* [Create unauthorized session](sessions/create.md) : `POST /v1/sessions`
* [Update session](sessions/update.md) : `PUT /v1/sessions`
* [Get session info](sessions/get.md) : `GET /v1/sessions`
