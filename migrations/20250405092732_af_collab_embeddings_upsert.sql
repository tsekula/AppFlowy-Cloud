-- Drop existing primary key if it exists:
ALTER TABLE af_collab_embeddings
DROP CONSTRAINT IF EXISTS af_collab_embeddings_pkey;

-- Add a new composite primary key on (fragment_id, oid):
-- Currently the fragment_id is generated by hash fragment content, so fragment_id might be
-- conflicting with other fragments, but they are not in the same document.
ALTER TABLE af_collab_embeddings
    ADD CONSTRAINT af_collab_embeddings_pkey
        PRIMARY KEY (fragment_id, oid);

CREATE OR REPLACE PROCEDURE af_collab_embeddings_upsert(
    IN p_workspace_id UUID,
    IN p_oid TEXT,
    IN p_tokens_used INT,
    IN p_fragments af_fragment_v3[]
)
LANGUAGE plpgsql
AS
$$
BEGIN
-- Delete all fragments for p_oid that are not present in the new fragment list.
DELETE
FROM af_collab_embeddings
WHERE oid = p_oid
  AND fragment_id NOT IN (
    SELECT fragment_id FROM UNNEST(p_fragments) AS f
);

-- Use MERGE to update existing rows or insert new ones without causing duplicate key errors.
MERGE INTO af_collab_embeddings AS t
    USING (
        SELECT
            f.fragment_id,
            p_oid AS oid,
            f.content_type,
            f.contents,
            f.embedding,
            NOW() AS indexed_at,
            f.metadata,
            f.fragment_index,
            f.embedder_type
        FROM UNNEST(p_fragments) AS f
    ) AS s
    ON t.oid = s.oid AND t.fragment_id = s.fragment_id
    WHEN MATCHED THEN -- this fragment has not changed
        UPDATE SET indexed_at = NOW()
    WHEN NOT MATCHED THEN -- this fragment is new
        INSERT (
                fragment_id,
                oid,
                content_type,
                content,
                embedding,
                indexed_at,
                metadata,
                fragment_index,
                embedder_type
            )
            VALUES (
                s.fragment_id,
                s.oid,
                s.content_type,
                s.contents,
                s.embedding,
                NOW(),
                s.metadata,
                s.fragment_index,
                s.embedder_type
            );

-- Update the usage tracking table with an upsert.
INSERT INTO af_workspace_ai_usage(
    created_at,
    workspace_id,
    search_requests,
    search_tokens_consumed,
    index_tokens_consumed
)
VALUES (
           NOW()::date,
           p_workspace_id,
           0,
           0,
           p_tokens_used
       )
    ON CONFLICT (created_at, workspace_id)
        DO UPDATE SET index_tokens_consumed = af_workspace_ai_usage.index_tokens_consumed + p_tokens_used;

END
$$;