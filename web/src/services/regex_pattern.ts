// Copyright 2023 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

import http from "./http";
type PatternPayload = {
  name: string;
  pattern: string;
  description: string;
};
const regexPatterns = {
  list: (
    org_identifier: string
  ) => {
    return http().get(
      `/api/${org_identifier}/re_patterns`
    );
  },
  create: (
    org_identifier: string,
    payload: PatternPayload
  ) => {
    return http().post(
      `/api/${org_identifier}/re_patterns`,
      payload
    );
  },
  update: (
    org_identifier: string,
    id: string,
    payload: any
  ) => {
    return http().put(
      `/api/${org_identifier}/re_patterns/${id}`,
      payload
    );
  },
  delete: (
    org_identifier: string,
    id: string,
  ) => {
    return http().delete(
      `/api/${org_identifier}/re_patterns/${id}`
    );
  },
  test: (
    org_identifier: string,
    pattern: string,
    test_records: Array<string>,
  ) => {
    return http().post(`/api/${org_identifier}/re_patterns/test`, { pattern, test_records });
  }
};

export default regexPatterns;
