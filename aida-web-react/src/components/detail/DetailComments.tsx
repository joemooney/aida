import { useState } from 'react';
import { Send } from 'lucide-react';
import type { Comment } from '@shared/types';
import { Avatar } from '../ui/Avatar';
import { formatRelativeDate } from '../../lib/utils';
import { addComment } from '../../api/requirements';
import { useQueryClient } from '@tanstack/react-query';

function CommentItem({ comment }: { comment: Comment }) {
  return (
    <div className="flex gap-3">
      <Avatar name={comment.author} size="sm" />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 mb-1">
          <span className="text-xs font-medium text-content">{comment.author}</span>
          <span className="text-[11px] text-content-muted">{formatRelativeDate(comment.created_at)}</span>
        </div>
        <div className="text-sm text-content-secondary leading-relaxed whitespace-pre-wrap">
          {comment.content}
        </div>
        {/* Nested replies */}
        {comment.replies && comment.replies.length > 0 && (
          <div className="mt-3 pl-4 border-l border-edge space-y-3">
            {comment.replies.map((reply) => (
              <CommentItem key={reply.id} comment={reply} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

interface DetailCommentsProps {
  requirementId: string;
  comments: Comment[];
}

export function DetailComments({ requirementId, comments }: DetailCommentsProps) {
  const [text, setText] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const queryClient = useQueryClient();

  async function handleSubmit() {
    if (!text.trim() || submitting) return;
    setSubmitting(true);
    try {
      await addComment(requirementId, text.trim());
      setText('');
      queryClient.invalidateQueries({ queryKey: ['requirement', requirementId] });
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="border-t border-edge px-6 py-4 shrink-0">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-3">
        Comments ({comments.length})
      </h3>

      {comments.length > 0 && (
        <div className="space-y-4 mb-4 max-h-60 overflow-y-auto">
          {comments.map((c) => (
            <CommentItem key={c.id} comment={c} />
          ))}
        </div>
      )}

      {/* Add comment */}
      <div className="flex gap-2">
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSubmit()}
          placeholder="Add a comment..."
          className="flex-1 rounded-lg border border-edge bg-surface px-3 py-1.5 text-sm text-content placeholder:text-content-muted focus:border-accent focus:outline-none"
        />
        <button
          onClick={handleSubmit}
          disabled={!text.trim() || submitting}
          className="flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-white hover:bg-accent-hover disabled:opacity-40 transition-colors cursor-pointer disabled:cursor-not-allowed"
        >
          <Send className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}
