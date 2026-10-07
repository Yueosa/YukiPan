-- 图床切片: 相册、图片指向、标签 (文档第 3、6 章)。
-- 图床没有目录树, 列表以库为准; image_refs 是「墙上这个名字指着这份内容」,
-- 引用计数 = private_refs + image_refs (+ 切片 4 的 guest_refs)。

CREATE TABLE albums (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);

-- 标签按用户隔离, name_norm 规范化 (trim + 折叠空白 + 小写) 后唯一。
CREATE TABLE tags (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    name_norm  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name_norm)
);

-- 公开文件名即盘上 public/images/ 下的名字 (server 生成, 与原名无关)。
CREATE TABLE image_refs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    album_id    UUID NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    public_name TEXT NOT NULL UNIQUE,
    orig_name   TEXT NOT NULL,
    sha256      TEXT NOT NULL REFERENCES blobs(sha256),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 引用计数与 TTL 清扫按 hash 反查指向。
CREATE INDEX image_refs_sha256_idx ON image_refs (sha256);

CREATE TABLE image_tags (
    image_id UUID NOT NULL REFERENCES image_refs(id) ON DELETE CASCADE,
    tag_id   UUID NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (image_id, tag_id)
);
